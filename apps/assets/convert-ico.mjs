import sharp from 'sharp';
import fs from 'fs';
import path from 'path';
import { fileURLToPath } from 'url';

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const srcDir = path.join(__dirname, 'icons');
const outDir = path.join(__dirname, '..', 'core', 'assets', 'icons');

fs.mkdirSync(outDir, { recursive: true });

const sizes = [16, 32, 48, 72];

function icoEncode(pngBuffers) {
  const header = Buffer.alloc(6);
  header.writeUInt16LE(0, 0);
  header.writeUInt16LE(1, 2);       // ICO type
  header.writeUInt16LE(pngBuffers.length, 4);

  let dataOffset = 6 + pngBuffers.length * 16;
  const entries = [];
  const dataList = [];

  for (const png of pngBuffers) {
    const w = png.readUInt8(16);
    const h = png.readUInt8(20);
    const entry = Buffer.alloc(16);
    entry.writeUInt8(w === 256 ? 0 : w, 0);
    entry.writeUInt8(h === 256 ? 0 : h, 1);
    entry.writeUInt8(0, 2);
    entry.writeUInt8(0, 3);
    entry.writeUInt16LE(1, 4);
    entry.writeUInt16LE(32, 6);
    entry.writeUInt32LE(png.length, 8);
    entry.writeUInt32LE(dataOffset, 12);
    entries.push(entry);
    dataList.push(png);
    dataOffset += png.length;
  }

  return Buffer.concat([header, ...entries, ...dataList]);
}

const files = fs.readdirSync(srcDir).filter(f => f.endsWith('.png'));
for (const file of files) {
  const pngs = await Promise.all(sizes.map(s =>
    sharp(path.join(srcDir, file))
      .resize(s, s, { fit: 'contain', background: { r: 0, g: 0, b: 0, alpha: 0 } })
      .png()
      .toBuffer()
  ));
  const icoName = file.replace('.png', '.ico');
  fs.writeFileSync(path.join(outDir, icoName), icoEncode(pngs));
  console.log('Created: apps/core/assets/icons/' + icoName);
}
