import qrcode from 'qrcode-generator'

/**
 * The QR encoder, and the two decisions it makes.
 *
 * The server sends a `qr_string` — the gateway's own payload, or the sandbox
 * mock's — and this module turns it into drawable modules. Nothing here composes,
 * wraps or re-formats the payload: a code this app invented would be a code the
 * gateway does not recognise, and the student would scan it and pay nothing.
 *
 * **Error correction is `M`** (about 15% recovery), which is the level QRIS
 * payloads are printed at and the one that still scans through a phone screen's
 * glare, a fingerprint on the glass and a camera at an angle. `L` would make a
 * denser code that fails on the first smudge; `H` would make it sparser than the
 * payload needs and push it to a higher version, i.e. more modules on a smaller
 * screen. `M` is the choice, and it is stated here rather than left to a default.
 *
 * **The byte encoding is UTF-8.** The library's default writes `charCodeAt`, which
 * is not UTF-8 and would corrupt any non-ASCII byte in the payload — a merchant
 * name, most plausibly. A QRIS payload is TLV of ASCII digits and tags, so this
 * changes nothing today; it is here so that the day a gateway sends a UTF-8
 * merchant name the code does not silently become unscannable.
 */

qrcode.stringToBytes = qrcode.stringToBytesFuncs['UTF-8'] ?? qrcode.stringToBytes

/** One QR code: its module grid size, and the path that draws its dark modules. */
export interface QrModules {
  /** Modules per side, excluding the quiet zone. */
  count: number
  /** An SVG path, in module units, one subpath per dark module. */
  path: string
}

/**
 * The payload as modules, or null when it cannot be encoded.
 *
 * Null rather than a throw: the encoder rejects a payload too long for version 40,
 * and the caller is a screen that polls — an exception inside a poll takes the
 * whole screen down, when what has actually happened is that the code cannot be
 * drawn. The amount, the state and the order reference are still on screen and
 * still true.
 *
 * The type number is `0`, which is the library's "choose the smallest version that
 * fits". That is the right direction for a code read by a camera at arm's length:
 * the smallest version is the largest module size for a given width.
 */
export function qrModules(payload: string): QrModules | null {
  try {
    const code = qrcode(0, 'M')
    code.addData(payload)
    code.make()

    const count = code.getModuleCount()
    const parts: string[] = []
    for (let row = 0; row < count; row += 1) {
      for (let col = 0; col < count; col += 1) {
        if (code.isDark(row, col)) parts.push(`M${col} ${row}h1v1h-1z`)
      }
    }
    return { count, path: parts.join('') }
  } catch {
    return null
  }
}
