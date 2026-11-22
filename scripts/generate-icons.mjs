import { mkdirSync, writeFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { deflateSync } from "node:zlib";

const __dirname = dirname(fileURLToPath(import.meta.url));
const ICONS_DIR = resolve(__dirname, "../src-tauri/icons");

// Standard IEEE 802.3 CRC-32 lookup table for PNG chunk checksums.
const CRC_TABLE = new Uint32Array(256);
for (let n = 0; n < 256; n++) {
  let c = n;
  for (let k = 0; k < 8; k++) {
    c = (c & 1) !== 0 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
  }
  CRC_TABLE[n] = c >>> 0;
}

function crc32(buf) {
  let crc = 0xffffffff;
  for (let i = 0; i < buf.length; i++) {
    crc = CRC_TABLE[(crc ^ buf[i]) & 0xff] ^ (crc >>> 8);
  }
  return (crc ^ 0xffffffff) >>> 0;
}

function makePngChunk(type, data) {
  const typeBytes = Buffer.from(type, "ascii");
  const len = Buffer.alloc(4);
  len.writeUInt32BE(data.length, 0);

  const crcInput = Buffer.concat([typeBytes, data]);
  const crcBuf = Buffer.alloc(4);
  crcBuf.writeUInt32BE(crc32(crcInput), 0);

  return Buffer.concat([len, typeBytes, data, crcBuf]);
}

function encodePng(width, height, rgba) {
  const signature = Buffer.from([137, 80, 78, 71, 13, 10, 26, 10]);

  const ihdr = Buffer.alloc(13);
  ihdr.writeUInt32BE(width, 0);
  ihdr.writeUInt32BE(height, 4);
  ihdr[8] = 8; // bit depth
  ihdr[9] = 6; // color type: RGBA
  ihdr[10] = 0; // compression: deflate
  ihdr[11] = 0; // filter: adaptive
  ihdr[12] = 0; // interlace: none

  const stride = width * 4;
  const raw = Buffer.alloc((stride + 1) * height);
  for (let y = 0; y < height; y++) {
    const rowStart = y * (stride + 1);
    raw[rowStart] = 0; // filter type 0 (None)
    rgba.copy(raw, rowStart + 1, y * stride, (y + 1) * stride);
  }

  const compressed = deflateSync(raw, { level: 9 });

  return Buffer.concat([
    signature,
    makePngChunk("IHDR", ihdr),
    makePngChunk("IDAT", compressed),
    makePngChunk("IEND", Buffer.alloc(0)),
  ]);
}

function encodeIcoDibEntry(width, height, rgba) {
  const header = Buffer.alloc(40);
  const xorSize = width * height * 4;
  const andRowBytes = Math.ceil(width / 32) * 4;
  const andSize = andRowBytes * height;

  header.writeUInt32LE(40, 0); // biSize
  header.writeInt32LE(width, 4); // biWidth
  header.writeInt32LE(height * 2, 8); // biHeight (XOR + AND masks)
  header.writeUInt16LE(1, 12); // biPlanes
  header.writeUInt16LE(32, 14); // biBitCount
  header.writeUInt32LE(0, 16); // biCompression (BI_RGB)
  header.writeUInt32LE(xorSize + andSize, 20); // biSizeImage

  const xorMask = Buffer.alloc(xorSize);
  const andMask = Buffer.alloc(andSize);

  for (let y = 0; y < height; y++) {
    const srcY = height - 1 - y; // BMP DIB is bottom-up
    for (let x = 0; x < width; x++) {
      const srcIdx = (srcY * width + x) * 4;
      const dstIdx = (y * width + x) * 4;
      const r = rgba[srcIdx];
      const g = rgba[srcIdx + 1];
      const b = rgba[srcIdx + 2];
      const a = rgba[srcIdx + 3];

      xorMask[dstIdx] = b;
      xorMask[dstIdx + 1] = g;
      xorMask[dstIdx + 2] = r;
      xorMask[dstIdx + 3] = a;

      if (a === 0) {
        const byteIdx = y * andRowBytes + (x >> 3);
        andMask[byteIdx] |= 0x80 >> (x & 7);
      }
    }
  }

  return Buffer.concat([header, xorMask, andMask]);
}

function encodeIco(images) {
  const count = images.length;
  const header = Buffer.alloc(6);
  header.writeUInt16LE(0, 0); // reserved
  header.writeUInt16LE(1, 2); // type: 1 = ICO
  header.writeUInt16LE(count, 4);

  const payloads = images.map(({ width, height, rgba }) =>
    encodeIcoDibEntry(width, height, rgba),
  );

  const dirEntries = Buffer.alloc(16 * count);
  let offset = 6 + 16 * count;

  for (let i = 0; i < count; i++) {
    const { width, height } = images[i];
    const data = payloads[i];
    const base = i * 16;

    dirEntries[base] = width >= 256 ? 0 : width;
    dirEntries[base + 1] = height >= 256 ? 0 : height;
    dirEntries[base + 2] = 0; // color count
    dirEntries[base + 3] = 0; // reserved
    dirEntries.writeUInt16LE(1, base + 4); // planes
    dirEntries.writeUInt16LE(32, base + 6); // bit count
    dirEntries.writeUInt32LE(data.length, base + 8); // bytes in resource
    dirEntries.writeUInt32LE(offset, base + 12); // image offset

    offset += data.length;
  }

  return Buffer.concat([header, dirEntries, ...payloads]);
}

// --- SDF Geometry Primitives (normalized coordinates u, v in [-1, 1]) ---

function sdRoundedRect(x, y, hx, hy, r) {
  const qx = Math.abs(x) - hx + r;
  const qy = Math.abs(y) - hy + r;
  const ox = Math.max(qx, 0);
  const oy = Math.max(qy, 0);
  return Math.hypot(ox, oy) + Math.min(Math.max(qx, qy), 0) - r;
}

function sdCircle(x, y, cx, cy, r) {
  return Math.hypot(x - cx, y - cy) - r;
}

function sdSegment(x, y, ax, ay, bx, by) {
  const pax = x - ax;
  const pay = y - ay;
  const bax = bx - ax;
  const bay = by - ay;
  const denom = bax * bax + bay * bay;
  const h = denom > 0 ? Math.max(0, Math.min(1, (pax * bax + pay * bay) / denom)) : 0;
  return Math.hypot(pax - bax * h, pay - bay * h);
}

// Convex polygon SDF (vertices in clockwise order, +Y downwards).
function sdConvexPolygon(x, y, verts) {
  let minDistSq = Infinity;
  let inside = true;

  for (let i = 0; i < verts.length; i++) {
    const [ax, ay] = verts[i];
    const [bx, by] = verts[(i + 1) % verts.length];
    const ex = bx - ax;
    const ey = by - ay;
    const px = x - ax;
    const py = y - ay;

    // Cross product for clockwise winding with +Y down: interior has ex*py - ey*px >= 0.
    if (ex * py - ey * px < 0) {
      inside = false;
    }

    const t = Math.max(0, Math.min(1, (px * ex + py * ey) / (ex * ex + ey * ey)));
    const dx = px - ex * t;
    const dy = py - ey * t;
    const dSq = dx * dx + dy * dy;
    if (dSq < minDistSq) {
      minDistSq = dSq;
    }
  }

  const dist = Math.sqrt(minDistSq);
  return inside ? -dist : dist;
}

const SHIELD_VERTS = [
  [-0.56, -0.56],
  [0.0, -0.68],
  [0.56, -0.56],
  [0.54, 0.04],
  [0.0, 0.68],
  [-0.54, 0.04],
];

function sdShield(x, y, scale = 1.0, offsetY = 0.0) {
  const sx = x / scale;
  const sy = (y - offsetY) / scale;
  return (sdConvexPolygon(sx, sy, SHIELD_VERTS) - 0.07) * scale;
}

function sdReticle(x, y, cy, dotR, tickInner, tickOuter, tickHalfW) {
  const dDot = sdCircle(x, y, 0, cy, dotR);
  const dTop = sdSegment(x, y, 0, cy - tickOuter, 0, cy - tickInner) - tickHalfW;
  const dBot = sdSegment(x, y, 0, cy + tickInner, 0, cy + tickOuter) - tickHalfW;
  const dLeft = sdSegment(x, y, -tickOuter, cy, -tickInner, cy) - tickHalfW;
  const dRight = sdSegment(x, y, tickInner, cy, tickOuter, cy) - tickHalfW;
  return Math.min(dDot, dTop, dBot, dLeft, dRight);
}

// Porter-Duff source-over composite (premultiplied-safe straight alpha).
function over(dst, src) {
  const sa = src[3];
  if (sa <= 0) return dst;
  const da = dst[3];
  const outA = sa + da * (1 - sa);
  if (outA <= 0) return [0, 0, 0, 0];
  return [
    (src[0] * sa + dst[0] * da * (1 - sa)) / outA,
    (src[1] * sa + dst[1] * da * (1 - sa)) / outA,
    (src[2] * sa + dst[2] * da * (1 - sa)) / outA,
    outA,
  ];
}

function renderIcon(size, sampleFn) {
  const rgba = Buffer.alloc(size * size * 4);
  const ss = 4; // 4x4 supersampling for crisp anti-aliased edges
  const invTotal = 1 / (ss * ss);

  for (let py = 0; py < size; py++) {
    for (let px = 0; px < size; px++) {
      let pr = 0;
      let pg = 0;
      let pb = 0;
      let pa = 0;

      for (let sy = 0; sy < ss; sy++) {
        const y = ((py + (sy + 0.5) / ss) / size) * 2 - 1;
        for (let sx = 0; sx < ss; sx++) {
          const x = ((px + (sx + 0.5) / ss) / size) * 2 - 1;
          const [r, g, b, a] = sampleFn(x, y);
          pr += r * a;
          pg += g * a;
          pb += b * a;
          pa += a;
        }
      }

      const outA = pa * invTotal;
      const idx = (py * size + px) * 4;
      if (outA > 1e-6) {
        rgba[idx] = Math.round(Math.min(255, (pr * invTotal) / outA));
        rgba[idx + 1] = Math.round(Math.min(255, (pg * invTotal) / outA));
        rgba[idx + 2] = Math.round(Math.min(255, (pb * invTotal) / outA));
        rgba[idx + 3] = Math.round(Math.min(255, outA * 255));
      }
    }
  }

  return rgba;
}

// --- Icon Artwork Definitions ---

// App icon: crisp white shield + reticle mark on #0A0A0A rounded square.
function sampleAppIcon(x, y) {
  let col = [0, 0, 0, 0];

  const bgDist = sdRoundedRect(x, y, 0.94, 0.94, 0.34);
  if (bgDist <= 0) {
    // Subtle #222224 border inset on the #0A0A0A rounded square.
    if (bgDist >= -0.045) {
      col = [0x22, 0x22, 0x24, 1];
    } else {
      col = [0x0a, 0x0a, 0x0a, 1];
    }
  }

  const shieldDist = sdShield(x, y, 0.88, 0.02);
  const shieldOutline = Math.abs(shieldDist + 0.055) - 0.065;
  const reticle = sdReticle(x, y, -0.02, 0.075, 0.16, 0.30, 0.04);

  if (Math.min(shieldOutline, reticle) <= 0) {
    col = over(col, [0xfa, 0xfa, 0xfa, 1]);
  }

  return col;
}

// Tray Standby: 32x32 monochrome shield outline + center dot (#D4D4D8).
function sampleTrayStandby(x, y) {
  const shieldDist = sdShield(x, y, 1.16, 0.0);
  const outline = Math.abs(shieldDist + 0.08) - 0.095;
  const dot = sdCircle(x, y, 0, -0.03, 0.14);

  if (Math.min(outline, dot) <= 0) {
    return [0xd4, 0xd4, 0xd8, 1];
  }
  return [0, 0, 0, 0];
}

// Tray Protected: 32x32 crisp white shield + #22C55E bottom-right status dot.
function sampleTrayProtected(x, y) {
  const dotCx = 0.56;
  const dotCy = 0.54;
  const dotR = 0.33;
  const haloR = 0.46;

  const dDot = sdCircle(x, y, dotCx, dotCy, dotR);
  if (dDot <= 0) {
    return [0x22, 0xc5, 0x5e, 1];
  }

  const dHalo = sdCircle(x, y, dotCx, dotCy, haloR);
  if (dHalo <= 0) {
    return [0, 0, 0, 0];
  }

  const shieldDist = sdShield(x, y, 1.16, 0.0);
  const centerCutout = sdCircle(x, y, 0, -0.04, 0.15);
  if (shieldDist <= 0 && centerCutout > 0) {
    return [0xfa, 0xfa, 0xfa, 1];
  }

  return [0, 0, 0, 0];
}

// Tray Paused: 32x32 dimmed #52525B hollow shield with crisp diagonal slash.
function sampleTrayPaused(x, y) {
  const slashDist = sdSegment(x, y, -0.74, -0.74, 0.74, 0.74);
  if (slashDist <= 0.10) {
    return [0x52, 0x52, 0x5b, 1];
  }
  // Clean separation gap around the diagonal slash.
  if (slashDist <= 0.21) {
    return [0, 0, 0, 0];
  }

  const shieldDist = sdShield(x, y, 1.16, 0.0);
  const outline = Math.abs(shieldDist + 0.08) - 0.095;
  if (outline <= 0) {
    return [0x52, 0x52, 0x5b, 1];
  }

  return [0, 0, 0, 0];
}

function main() {
  mkdirSync(ICONS_DIR, { recursive: true });

  const app16 = renderIcon(16, sampleAppIcon);
  const app32 = renderIcon(32, sampleAppIcon);
  const app48 = renderIcon(48, sampleAppIcon);
  const app128 = renderIcon(128, sampleAppIcon);

  const trayStandby = renderIcon(32, sampleTrayStandby);
  const trayProtected = renderIcon(32, sampleTrayProtected);
  const trayPaused = renderIcon(32, sampleTrayPaused);

  const outputs = [
    ["32x32.png", encodePng(32, 32, app32)],
    ["128x128.png", encodePng(128, 128, app128)],
    [
      "icon.ico",
      encodeIco([
        { width: 16, height: 16, rgba: app16 },
        { width: 32, height: 32, rgba: app32 },
        { width: 48, height: 48, rgba: app48 },
        { width: 128, height: 128, rgba: app128 },
      ]),
    ],
    ["tray-standby.png", encodePng(32, 32, trayStandby)],
    ["tray-protected.png", encodePng(32, 32, trayProtected)],
    ["tray-paused.png", encodePng(32, 32, trayPaused)],
  ];

  for (const [name, buf] of outputs) {
    const outPath = resolve(ICONS_DIR, name);
    writeFileSync(outPath, buf);
    console.log(`Generated ${name} (${buf.length} bytes)`);
  }
}

main();
