import { Socket } from "net";
import { AmbientFsError } from "./errors.js";

export interface JsonRpcRequest {
  jsonrpc: "2.0";
  method: string;
  params?: Record<string, unknown> | unknown[];
  id: number;
}

export interface JsonRpcResponse {
  jsonrpc: "2.0";
  result?: unknown;
  error?: {
    code: number;
    message: string;
    data?: unknown;
  };
  id: number;
}

export interface JsonRpcNotification {
  jsonrpc: "2.0";
  method: string;
  params?: Record<string, unknown> | unknown[];
}

export class Transport {
  private socket: Socket | null = null;
  private requestId = 1;
  private pending = new Map<number, {
    resolve: (value: unknown) => void;
    reject: (error: Error) => void;
    timeout: NodeJS.Timeout;
  }>();
  private notificationCallback: ((notification: JsonRpcNotification) => void) | null = null;
  private buffer = "";

  constructor(private socketPath: string) {}

  connect(): Promise<void> {
    return new Promise((resolve, reject) => {
      this.socket = new Socket();

      const timeout = setTimeout(() => {
        this.socket?.destroy();
        reject(AmbientFsError.timeout());
      }, 5000);

      this.socket.once("connect", () => {
        clearTimeout(timeout);
        resolve();
      });

      this.socket.once("error", (err) => {
        clearTimeout(timeout);
        reject(new AmbientFsError(`connect failed: ${err.message}`, "NOT_CONNECTED"));
      });

      this.socket.on("data", (data: Buffer) => this.handleData(data));
      this.socket.connect(this.socketPath);
    });
  }

  close(): void {
    // clear all pending
    for (const { timeout, reject } of this.pending.values()) {
      clearTimeout(timeout);
      reject(AmbientFsError.notConnected());
    }
    this.pending.clear();
    this.socket?.destroy();
    this.socket = null;
  }

  setNotificationCallback(cb: (notification: JsonRpcNotification) => void): void {
    this.notificationCallback = cb;
  }

  async request<T = unknown>(method: string, params?: Record<string, unknown>): Promise<T> {
    if (!this.socket) {
      throw AmbientFsError.notConnected();
    }

    const id = this.requestId++;
    const request: JsonRpcRequest = { jsonrpc: "2.0", method, params, id };

    return new Promise((resolve, reject) => {
      const timeout = setTimeout(() => {
        this.pending.delete(id);
        reject(AmbientFsError.timeout());
      }, 30000); // 30s default

      this.pending.set(id, { resolve, reject, timeout });

      const json = JSON.stringify(request) + "\n";
      this.socket!.write(json, (err) => {
        if (err) {
          clearTimeout(timeout);
          this.pending.delete(id);
          reject(new AmbientFsError(`write failed: ${err.message}`, "NOT_CONNECTED"));
        }
      });
    });
  }

  private handleData(data: Buffer): void {
    this.buffer += data.toString("utf-8");
    const lines = this.buffer.split("\n");
    this.buffer = lines.pop() || ""; // keep partial

    for (const line of lines) {
      if (!line.trim()) continue;
      try {
        const msg = JSON.parse(line);
        if ("id" in msg && typeof msg.id === "number") {
          this.handleResponse(msg as JsonRpcResponse);
        } else if ("method" in msg) {
          this.notificationCallback?.(msg as JsonRpcNotification);
        }
      } catch (e) {
        // log but don't crash
        console.error("failed to parse json-rpc message:", e);
      }
    }
  }

  private handleResponse(response: JsonRpcResponse): void {
    const pending = this.pending.get(response.id);
    if (!pending) return;

    this.pending.delete(response.id);
    clearTimeout(pending.timeout);

    if (response.error) {
      pending.reject(AmbientFsError.daemonError(response.error.message, response.error.code));
    } else {
      pending.resolve(response.result);
    }
  }

  isConnected(): boolean {
    return this.socket?.readyState === "open";
  }
}
