import type { ProtocolError, ProtocolErrorCode, RequestId, Request } from "./protocol";

export class BrowserKitRequestError extends Error {
  readonly code: ProtocolErrorCode | "timeout" | "transport";
  readonly requestId: RequestId;
  readonly requestType: Request["type"];

  constructor(
    error: ProtocolError | { code: "timeout" | "transport"; message: string },
    requestId: RequestId,
    requestType: Request["type"],
  ) {
    super(error.message);
    this.name = "BrowserKitRequestError";
    this.code = error.code;
    this.requestId = requestId;
    this.requestType = requestType;
  }
}

export class BrowserKitProtocolError extends Error {
  constructor(message: string) {
    super(message);
    this.name = "BrowserKitProtocolError";
  }
}
