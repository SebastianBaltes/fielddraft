// wu2v: uploaded-font byte retention + re-registration tests.
//
// opentype.js is mocked so we can register/parse fonts from arbitrary
// dummy bytes without shipping a real .ttf into the test.
import { describe, expect, it, vi } from "vitest";

vi.mock("opentype.js", () => {
  return {
    default: {
      // parse echoes a marker carrying the byte length so we can assert the
      // re-registered font came from the right bytes.
      parse: (buf: ArrayBuffer) => ({ __parsed: true, len: buf.byteLength }),
    },
  };
});

import {
  registerUploadedFont,
  getUploadedFontBytes,
  registerUploadedFontFromBytes,
  cachedFont,
  UPLOADED_FONT_PREFIX,
} from "./sketchTextFont";

describe("uploaded font byte retention (wu2v)", () => {
  it("retains the raw bytes passed to registerUploadedFont", () => {
    const bytes = new Uint8Array([10, 20, 30]);
    const id = registerUploadedFont("Foo.ttf", { __parsed: true } as never, bytes);
    expect(id).toBe(`${UPLOADED_FONT_PREFIX}Foo.ttf`);
    expect(getUploadedFontBytes(id)).toBe(bytes);
  });

  it("returns undefined for fonts registered without bytes", () => {
    const id = registerUploadedFont("Bare.ttf", { __parsed: true } as never);
    expect(getUploadedFontBytes(id)).toBeUndefined();
  });

  it("re-registers an uploaded font from persisted bytes (parse + cache + retain)", () => {
    const id = `${UPLOADED_FONT_PREFIX}Reloaded.ttf`;
    const bytes = new Uint8Array([1, 2, 3, 4]);
    registerUploadedFontFromBytes(id, bytes);

    // The parsed font is now cached under the verbatim id…
    const font = cachedFont(id) as unknown as { __parsed: boolean; len: number };
    expect(font).toBeDefined();
    expect(font.__parsed).toBe(true);
    expect(font.len).toBe(4);
    // …and the bytes are retained for a subsequent re-save.
    expect(getUploadedFontBytes(id)).toBe(bytes);
  });
});
