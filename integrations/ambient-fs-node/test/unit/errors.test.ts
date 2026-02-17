/**
 * unit tests for error handling
 */

import { AmbientFsError, ErrorCode } from "../../src/errors.js";

describe("AmbientFsError", () => {
  describe("constructor", () => {
    it("should create error with message and code", () => {
      const error = new AmbientFsError("test error", "NOT_CONNECTED");

      expect(error.message).toBe("test error");
      expect(error.code).toBe("NOT_CONNECTED");
      expect(error.name).toBe("AmbientFsError");
      expect(error instanceof Error).toBe(true);
    });

    it("should include details", () => {
      const details = { daemonCode: -32600 };
      const error = new AmbientFsError("daemon error", "DAEMON_ERROR", details);

      expect(error.details).toEqual(details);
    });

    it("should be throwable and catchable", () => {
      try {
        throw new AmbientFsError("test", "TIMEOUT");
      } catch (e) {
        expect(e).toBeInstanceOf(AmbientFsError);
        expect((e as AmbientFsError).code).toBe("TIMEOUT");
      }
    });
  });

  describe("static factories", () => {
    it("should create NOT_CONNECTED error", () => {
      const error = AmbientFsError.notConnected();

      expect(error.code).toBe("NOT_CONNECTED");
      expect(error.message).toBe("daemon not connected");
    });

    it("should create TIMEOUT error", () => {
      const error = AmbientFsError.timeout();

      expect(error.code).toBe("TIMEOUT");
      expect(error.message).toBe("operation timed out");
    });

    it("should create DAEMON_ERROR with message", () => {
      const error = AmbientFsError.daemonError("invalid params", -32602);

      expect(error.code).toBe("DAEMON_ERROR");
      expect(error.message).toBe("daemon returned error: invalid params");
      expect(error.details).toEqual({ daemonCode: -32602 });
    });

    it("should create DAEMON_ERROR without code", () => {
      const error = AmbientFsError.daemonError("some error");

      expect(error.code).toBe("DAEMON_ERROR");
      expect(error.details).toEqual({ daemonCode: undefined });
    });

    it("should create INVALID_RESPONSE error", () => {
      const error = AmbientFsError.invalidResponse("missing field");

      expect(error.code).toBe("INVALID_RESPONSE");
      expect(error.message).toBe("invalid response from daemon: missing field");
    });

    it("should create PARSE_ERROR", () => {
      const error = AmbientFsError.parseError("unexpected token");

      expect(error.code).toBe("PARSE_ERROR");
      expect(error.message).toBe("json parse error: unexpected token");
    });
  });

  describe("ErrorCode type", () => {
    it("should accept all valid error codes", () => {
      const codes: ErrorCode[] = [
        "NOT_CONNECTED",
        "TIMEOUT",
        "DAEMON_ERROR",
        "INVALID_RESPONSE",
        "PARSE_ERROR",
      ];

      codes.forEach((code) => {
        const error = new AmbientFsError("test", code);
        expect(error.code).toBe(code);
      });
    });
  });

  describe("error handling patterns", () => {
    it("should allow switch on error code", () => {
      const error = AmbientFsError.notConnected();

      let handled = false;
      switch (error.code) {
        case "NOT_CONNECTED":
          handled = true;
          break;
        case "TIMEOUT":
          handled = false;
          break;
        default:
          handled = false;
      }

      expect(handled).toBe(true);
    });

    it("should allow instanceof check", () => {
      const error = AmbientFsError.timeout();

      try {
        throw error;
      } catch (e) {
        if (e instanceof AmbientFsError) {
          expect(e.code).toBe("TIMEOUT");
        } else {
          fail("should be AmbientFsError");
        }
      }
    });
  });
});
