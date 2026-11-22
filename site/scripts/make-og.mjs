// Makes public/og.png, the image a link preview shows (1200x630): the headline over the real window screenshot.
// Run after the hero screenshot changes:  node scripts/make-og.mjs
import { createRequire } from 'node:module';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const here = path.dirname(fileURLToPath(import.meta.url));
const site = path.resolve(here, '..');
const require = createRequire(path.join(site, 'package.json'));
const sharp = require('sharp');

const W = 1200;
const H = 630;
const shot = await sharp(path.join(site, 'src', 'assets', 'shots', 'hero.png')).resize({ width: 1020 }).png().toBuffer();
const meta = await sharp(shot).metadata();

const svg = `
<svg xmlns="http://www.w3.org/2000/svg" width="${W}" height="${H}">
  <rect width="${W}" height="${H}" fill="#0a0d10"/>
  <g font-family="Segoe UI, Arial, sans-serif">
    <text x="64" y="104" font-size="26" font-weight="600" fill="#a9b5bf" letter-spacing="1">BRIDLE  ·  LOCAL-FIRST CODING AGENT FOR WINDOWS</text>
    <text x="64" y="188" font-size="76" font-weight="700" fill="#e8edf1" letter-spacing="-2">The model proposes.</text>
    <text x="64" y="268" font-size="76" font-weight="700" fill="#62c6f2" letter-spacing="-2">Bridle decides.</text>
  </g>
</svg>`;

const frameTop = 320;
const visible = Math.min(meta.height ?? 0, H - frameTop + 40);
const cropped = await sharp(shot).extract({ left: 0, top: 0, width: meta.width ?? 1020, height: visible }).png().toBuffer();

await sharp({ create: { width: W, height: H, channels: 3, background: '#0a0d10' } })
  .composite([
    { input: Buffer.from(svg), left: 0, top: 0 },
    { input: cropped, left: 90, top: frameTop },
  ])
  .png({ compressionLevel: 9 })
  .toFile(path.join(site, 'public', 'og.png'));
console.log('wrote public/og.png');
