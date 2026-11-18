// THROWAWAY (target spike). The least PNG reader: 8 bits, RGB or RGBA, not interlaced. Gives RGB.

import zlib from 'node:zlib';

export function decode(png) {
  let at = 8;
  let width, height, channels;
  const parts = [];
  while (at < png.length) {
    const len = png.readUInt32BE(at);
    const type = png.toString('latin1', at + 4, at + 8);
    const data = png.subarray(at + 8, at + 8 + len);
    if (type === 'IHDR') {
      width = data.readUInt32BE(0);
      height = data.readUInt32BE(4);
      const [depth, colour, , , interlace] = data.subarray(8);
      channels = { 2: 3, 6: 4 }[colour];
      if (depth !== 8 || !channels || interlace) throw new Error(`a PNG this reader does not read: depth ${depth}, colour ${colour}`);
    } else if (type === 'IDAT') parts.push(data);
    at += 12 + len;
  }
  const raw = zlib.inflateSync(Buffer.concat(parts));
  const stride = width * channels;
  const rows = Buffer.alloc(height * stride);
  for (let y = 0; y < height; y++) {
    const filter = raw[y * (stride + 1)];
    const line = raw.subarray(y * (stride + 1) + 1, (y + 1) * (stride + 1));
    for (let i = 0; i < stride; i++) {
      const a = i >= channels ? rows[y * stride + i - channels] : 0;
      const b = y ? rows[(y - 1) * stride + i] : 0;
      const c = y && i >= channels ? rows[(y - 1) * stride + i - channels] : 0;
      let v = line[i];
      if (filter === 1) v += a;
      else if (filter === 2) v += b;
      else if (filter === 3) v += (a + b) >> 1;
      else if (filter === 4) {
        const p = a + b - c;
        const [pa, pb, pc] = [Math.abs(p - a), Math.abs(p - b), Math.abs(p - c)];
        v += pa <= pb && pa <= pc ? a : pb <= pc ? b : c;
      }
      rows[y * stride + i] = v & 255;
    }
  }
  const rgb = Buffer.alloc(width * height * 3);
  for (let i = 0, o = 0; i < rows.length; i += channels, o += 3) rows.copy(rgb, o, i, i + 3);
  return { width, height, rgb };
}

/// A rectangle cut out of a picture.
export function crop(img, { x, y, w, h }) {
  const rgb = Buffer.alloc(w * h * 3);
  for (let r = 0; r < h; r++) img.rgb.copy(rgb, r * w * 3, ((y + r) * img.width + x) * 3, ((y + r) * img.width + x + w) * 3);
  return { width: w, height: h, rgb };
}

/// Every place `ref` shows in `img`, to within `tol` (mean difference of a channel, 0 to 255), best first. Places a
/// few pixels apart are one place.
export function find(img, ref, tol = 6) {
  // Probe first at the reference's least ordinary pixels, so plain background is thrown out at once.
  const n = ref.width * ref.height;
  const bg = [ref.rgb[0], ref.rgb[1], ref.rgb[2]];
  const odd = [];
  for (let i = 0; i < n; i++) {
    const d = Math.abs(ref.rgb[i * 3] - bg[0]) + Math.abs(ref.rgb[i * 3 + 1] - bg[1]) + Math.abs(ref.rgb[i * 3 + 2] - bg[2]);
    if (d > 120) odd.push(i);
  }
  const probes = [];
  for (let k = 0; k < 24 && odd.length; k++) probes.push(odd[Math.floor((k * odd.length) / 24)]);
  const limit = tol * n * 3;
  const hits = [];
  for (let y = 0; y + ref.height <= img.height; y++) {
    next: for (let x = 0; x + ref.width <= img.width; x++) {
      for (const i of probes) {
        const at = ((y + Math.floor(i / ref.width)) * img.width + x + (i % ref.width)) * 3;
        if (Math.abs(img.rgb[at] - ref.rgb[i * 3]) + Math.abs(img.rgb[at + 1] - ref.rgb[i * 3 + 1]) + Math.abs(img.rgb[at + 2] - ref.rgb[i * 3 + 2]) > 150) continue next;
      }
      let sum = 0;
      for (let r = 0; r < ref.height; r++) {
        const a = ((y + r) * img.width + x) * 3;
        const b = r * ref.width * 3;
        for (let i = 0; i < ref.width * 3; i++) sum += Math.abs(img.rgb[a + i] - ref.rgb[b + i]);
        if (sum > limit) continue next;
      }
      hits.push({ x, y, off: +(sum / (n * 3)).toFixed(2) });
    }
  }
  hits.sort((a, b) => a.off - b.off);
  const places = [];
  for (const h of hits) if (!places.some((p) => Math.abs(p.x - h.x) < 10 && Math.abs(p.y - h.y) < 10)) places.push(h);
  return places;
}
