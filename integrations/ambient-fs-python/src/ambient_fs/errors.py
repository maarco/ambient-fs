"""Exception hierarchy for ambient-fs client."""


class AmbientFsError(Exception):
    """Base exception for all client errors."""


class ConnectionError(AmbientFsError):
    """Failed to connect to daemon socket."""


class TimeoutError(AmbientFsError):
    """Request timed out."""


class DaemonError(AmbientFsError):
    """Daemon returned an error response.

    Attributes:
        code: JSON-RPC error code
        message: Error message from daemon
    """

    def __init__(self, code: int, message: str) -> None:
        self.code = code
        self.message = message
        super().__init__(f"daemon error {code}: {message}")


class InvalidResponseError(AmbientFsError):
    """Couldn't parse daemon response."""
