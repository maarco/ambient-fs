export class AmbientFsError extends Error {
  constructor(
    message: string,
    public code: ErrorCode,
    public details?: unknown
  ) {
    super(message);
    this.name = "AmbientFsError";
  }

  static notConnected(): AmbientFsError {
    return new AmbientFsError("daemon not connected", "NOT_CONNECTED");
  }

  static timeout(): AmbientFsError {
    return new AmbientFsError("operation timed out", "TIMEOUT");
  }

  static daemonError(message: string, code?: number): AmbientFsError {
    return new AmbientFsError(
      `daemon returned error: ${message}`,
      "DAEMON_ERROR",
      { daemonCode: code }
    );
  }

  static invalidResponse(message: string): AmbientFsError {
    return new AmbientFsError(`invalid response from daemon: ${message}`, "INVALID_RESPONSE");
  }

  static parseError(message: string): AmbientFsError {
    return new AmbientFsError(`json parse error: ${message}`, "PARSE_ERROR");
  }
}

export type ErrorCode =
  | "NOT_CONNECTED"
  | "TIMEOUT"
  | "DAEMON_ERROR"
  | "INVALID_RESPONSE"
  | "PARSE_ERROR";
