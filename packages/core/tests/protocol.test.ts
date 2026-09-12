import { describe, expect, it } from "vitest";
import { PROTOCOL_VERSION, parseNativeMessage } from "../src/index";

describe("protocol", () => {
  it("matches Rust M0-D protocol version", () => expect(PROTOCOL_VERSION).toBe(1));
  it("rejects malformed native messages", () => expect(() => parseNativeMessage({ type: "nope" })).toThrow());
});
