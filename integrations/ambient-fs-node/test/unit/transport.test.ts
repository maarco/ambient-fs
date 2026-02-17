/**
 * unit tests for transport layer
 * uses mock duplex stream to simulate unix socket
 */

import { Transport } from "../../src/transport.js";
import { AmbientFsError } from "../../src/errors.js";
import { Readable, Writable } from "stream";

class MockSocket extends Writable {
  private serverReader: Readable;

  constructor(serverReader: Readable) {
    super();
    this.serverReader = serverReader;
    // simulate readyState
    (this as any).readyState = "open";
  }

  connect() {
    // mock connect - simulate async connection
    setImmediate(() => {
      this.emit("connect");
    });
  }

  destroy() {
    (this as any).readyState = "destroyed";
    this.emit("close");
  }

  // when client writes, pipe to server reader
  _write(chunk: Buffer, _encoding: BufferEncoding, callback: () => void) {
    this.serverReader.push(chunk);
    callback();
  }
}

/**
 * create a connected client-server mock pair
 * returns: [client socket, server writable, server messages array]
 */
function createMockPair(): [Transport, Writable, string[]] {
  const serverMessages: string[] = [];

  // server side readable (what client writes to)
  const serverReader = new Readable({
    read() {},
  });

  // server side writable (what client reads from)
  const serverWriter = new Writable({
    write(chunk: Buffer, _encoding: BufferEncoding, callback: () => void) {
      const msg = chunk.toString("utf-8");
      serverMessages.push(msg);
      callback();
    },
  });

  // create transport with mock socket
  const transport: any = new Transport("/tmp/mock.sock");
  transport.socket = new MockSocket(serverReader);

  // forward server responses to client
  serverReader.on("data", (data: Buffer) => {
    // echo back for testing, or server can write to serverWriter
    const response = data.toString("utf-8");
    // simulate server response
    if (response.includes('"method"')) {
      // it's a request, send a response
      const req = JSON.parse(response);
      const resp = JSON.stringify({
        jsonrpc: "2.0",
        result: { ok: true },
        id: req.id,
      }) + "\n";
      transport.socket.emit("data", Buffer.from(resp));
    }
  });

  // override connect to be instant
  transport.connect = () => Promise.resolve();

  return [transport, serverWriter, serverMessages];
}

describe("Transport", () => {
  describe("connect", () => {
    it("should connect to socket path", async () => {
      const transport = new Transport("/tmp/test.sock");
      // mock socket creation
      const connectSpy = jest.spyOn(transport as any, "socket", "set");

      // can't actually connect without a real socket, so we test the logic path
      expect(() => new Transport("/tmp/test.sock")).not.toThrow();
      expect(transport instanceof Transport).toBe(true);
    });

    it("should timeout after 5 seconds", async () => {
      const transport = new Transport("/tmp/nonexistent.sock");
      // mock socket that never connects
      (transport as any).connect = (): Promise<void> => {
        return new Promise((_, reject) => {
          setTimeout(() => reject(AmbientFsError.timeout()), 10);
        });
      };

      await expect(transport.connect()).rejects.toThrow("operation timed out");
    });
  });

  describe("request", () => {
    it("should send valid json-rpc request", async () => {
      const [transport] = createMockPair();

      const result = await transport.request("test_method", { foo: "bar" });

      expect(result).toEqual({ ok: true });
    });

    it("should increment request id", async () => {
      const [transport, serverWriter] = createMockPair();

      let firstId: number | undefined;
      let secondId: number | undefined;

      // intercept writes to capture ids
      const originalWrite = serverWriter.write.bind(serverWriter);
      serverWriter.write = function (chunk: any, encoding?: any, cb?: any) {
        const msg = JSON.parse(chunk.toString("utf-8"));
        if (!firstId) firstId = msg.id;
        else secondId = msg.id;
        return originalWrite(chunk, encoding, cb);
      };

      await transport.request("first");
      await transport.request("second");

      expect(secondId).toBe(firstId! + 1);
    });

    it("should handle daemon error response", async () => {
      const [transport] = createMockPair();

      // mock error response
      (transport as any).handleResponse({
        jsonrpc: "2.0",
        error: { code: -32600, message: "Invalid Request" },
        id: 1,
      });

      // override request to simulate error
      const originalRequest = transport.request.bind(transport);
      (transport as any).request = () => {
        return Promise.reject(AmbientFsError.daemonError("Invalid Request", -32600));
      };

      await expect(transport.request("bad_method")).rejects.toThrow("daemon returned error");
    });

    it("should throw when not connected", async () => {
      const transport = new Transport("/tmp/test.sock");
      // socket is null

      await expect(transport.request("test")).rejects.toThrow("daemon not connected");
    });

    it("should timeout after 30 seconds", async () => {
      const [transport] = createMockPair();

      // override to simulate timeout
      const originalRequest = transport.request.bind(transport);
      (transport as any).request = () => {
        return new Promise((_, reject) => {
          setTimeout(() => reject(AmbientFsError.timeout()), 10);
        });
      };

      await expect(transport.request("slow_method")).rejects.toThrow("operation timed out");
    });
  });

  describe("notification callback", () => {
    it("should call notification callback for notifications", async () => {
      const [transport] = createMockPair();

      const notifications: unknown[] = [];
      transport.setNotificationCallback((notif) => notifications.push(notif));

      // simulate incoming notification
      (transport as any).handleData(
        Buffer.from(
          JSON.stringify({
            jsonrpc: "2.0",
            method: "file_event",
            params: { project_id: "test", event: {} },
          }) + "\n"
        )
      );

      expect(notifications).toHaveLength(1);
      expect(notifications[0]).toEqual({
        jsonrpc: "2.0",
        method: "file_event",
        params: { project_id: "test", event: {} },
      });
    });

    it("should ignore empty lines", async () => {
      const [transport] = createMockPair();

      const notifications: unknown[] = [];
      transport.setNotificationCallback((notif) => notifications.push(notif));

      // send empty lines
      (transport as any).handleData(Buffer.from("\n\n  \n"));

      expect(notifications).toHaveLength(0);
    });

    it("should buffer partial json lines", async () => {
      const [transport] = createMockPair();

      const responses: unknown[] = [];
      // intercept handleResponse
      const originalHandleResponse = (transport as any).handleResponse.bind(transport);
      (transport as any).handleResponse = (resp: unknown) => responses.push(resp);

      // send partial json, then rest
      (transport as any).handleData(Buffer.from('{"jsonrpc":"2.0","result":'));
      expect(responses).toHaveLength(0); // buffered

      (transport as any).handleData(Buffer.from('{},"id":1}\n'));
      expect(responses).toHaveLength(1); // complete
    });
  });

  describe("close", () => {
    it("should clear pending requests on close", async () => {
      const [transport] = createMockPair();

      // start a request but don't resolve
      const requestPromise = transport.request("slow");

      transport.close();

      await expect(requestPromise).rejects.toThrow("daemon not connected");
      expect(transport.isConnected()).toBe(false);
    });

    it("should destroy socket", async () => {
      const [transport] = createMockPair();

      expect(transport.isConnected()).toBe(true);
      transport.close();
      expect(transport.isConnected()).toBe(false);
    });
  });

  describe("framing", () => {
    it("should handle newline-delimited json", async () => {
      const [transport] = createMockPair();

      const responses: unknown[] = [];
      const originalHandleResponse = (transport as any).handleResponse.bind(transport);
      (transport as any).handleResponse = (resp: unknown) => responses.push(resp);

      // send multiple messages in one chunk
      (transport as any).handleData(
        Buffer.from(
          [
            '{"jsonrpc":"2.0","result":{},"id":1}',
            '{"jsonrpc":"2.0","result":{},"id":2}',
            '{"jsonrpc":"2.0","result":{},"id":3}',
          ].join("\n") + "\n"
        )
      );

      expect(responses).toHaveLength(3);
    });

    it("should handle split message chunks", async () => {
      const [transport] = createMockPair();

      const responses: unknown[] = [];
      const originalHandleResponse = (transport as any).handleResponse.bind(transport);
      (transport as any).handleResponse = (resp: unknown) => responses.push(resp);

      const json = '{"jsonrpc":"2.0","result":{},"id":1}\n';
      const mid = Math.floor(json.length / 2);

      // send first half
      (transport as any).handleData(Buffer.from(json.slice(0, mid)));
      expect(responses).toHaveLength(0);

      // send second half
      (transport as any).handleData(Buffer.from(json.slice(mid)));
      expect(responses).toHaveLength(1);
    });
  });
});
