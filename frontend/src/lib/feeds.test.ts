import { describe, expect, it } from "vitest";
import { isNewSince, parseLastSeen } from "./feeds";

describe("parseLastSeen", () => {
  it("parses plain epoch seconds", () => {
    expect(parseLastSeen("1755500000")).toBe(1755500000);
  });
  it("strips the JSON quotes a settings_get returns", () => {
    expect(parseLastSeen('"1755500000"')).toBe(1755500000);
  });
  it("returns 0 for missing, invalid, or non-positive values (show-all-new default)", () => {
    expect(parseLastSeen(null)).toBe(0);
    expect(parseLastSeen(undefined)).toBe(0);
    expect(parseLastSeen("")).toBe(0);
    expect(parseLastSeen("nope")).toBe(0);
    expect(parseLastSeen("0")).toBe(0);
    expect(parseLastSeen("-5")).toBe(0);
  });
});

describe("isNewSince", () => {
  it("marks a catalog row newer than the last-seen threshold as new", () => {
    expect(isNewSince(1755501000, 1755500000)).toBe(true);
    expect(isNewSince(1755500000, 1755500000)).toBe(false);
    expect(isNewSince(1749999999, 1755500000)).toBe(false);
  });
});
