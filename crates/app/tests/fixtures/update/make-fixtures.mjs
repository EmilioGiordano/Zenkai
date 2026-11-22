// Writes the signed update fixtures the updater tests read. TEST keys only: their secret
// seeds are below on purpose, and their public halves are compiled into test builds alone.
// Run with `node make-fixtures.mjs` from this folder; the output is deterministic.
import { createHash, createPrivateKey, createPublicKey, sign } from 'node:crypto';
import { writeFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const here = dirname(fileURLToPath(import.meta.url));
const PKCS8_ED25519 = Buffer.from('302e020100300506032b657004220420', 'hex');

function testKey(label) {
  const seed = createHash('sha256').update(`zenkai TEST key ${label}`).digest();
  const id = createHash('sha256').update(`zenkai TEST key id ${label}`).digest().subarray(0, 8);
  const secret = createPrivateKey({ key: Buffer.concat([PKCS8_ED25519, seed]), format: 'der', type: 'pkcs8' });
  const spki = createPublicKey(secret).export({ format: 'der', type: 'spki' });
  const raw = spki.subarray(spki.length - 32);
  const publicKey = Buffer.concat([Buffer.from('Ed'), id, raw]).toString('base64');
  return { label, id, secret, publicKey };
}

function minisig(content, key, trustedComment, { legacy = false, keyId = key.id } = {}) {
  const signed = legacy ? content : createHash('blake2b512').update(content).digest();
  const signature = sign(null, signed, key.secret);
  const algorithm = Buffer.from(legacy ? 'Ed' : 'ED');
  const global = sign(null, Buffer.concat([signature, Buffer.from(trustedComment)]), key.secret);
  return [
    `untrusted comment: signature from the zenkai TEST key ${key.label}`,
    Buffer.concat([algorithm, keyId, signature]).toString('base64'),
    `trusted comment: ${trustedComment}`,
    global.toString('base64'),
    '',
  ].join('\n');
}

const CRC_TABLE = Array.from({ length: 256 }, (_, n) => {
  let c = n;
  for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
  return c >>> 0;
});

function crc32(bytes) {
  let crc = 0xffffffff;
  for (const byte of bytes) crc = CRC_TABLE[(crc ^ byte) & 0xff] ^ (crc >>> 8);
  return (crc ^ 0xffffffff) >>> 0;
}

// Stored entries with fixed dates, so the bytes and the signatures never change.
function zip(entries) {
  const locals = [];
  const centrals = [];
  let offset = 0;
  for (const [name, text] of entries) {
    const nameBytes = Buffer.from(name);
    const body = Buffer.from(text ?? '');
    const crc = crc32(body);
    const local = Buffer.alloc(30);
    local.writeUInt32LE(0x04034b50, 0);
    local.writeUInt16LE(20, 4);
    local.writeUInt16LE(0x5821, 12);
    local.writeUInt16LE(0x5b75, 10);
    local.writeUInt32LE(crc, 14);
    local.writeUInt32LE(body.length, 18);
    local.writeUInt32LE(body.length, 22);
    local.writeUInt16LE(nameBytes.length, 26);
    const central = Buffer.alloc(46);
    central.writeUInt32LE(0x02014b50, 0);
    central.writeUInt16LE(20, 4);
    central.writeUInt16LE(20, 6);
    central.writeUInt16LE(0x5b75, 12);
    central.writeUInt16LE(0x5821, 14);
    central.writeUInt32LE(crc, 16);
    central.writeUInt32LE(body.length, 20);
    central.writeUInt32LE(body.length, 24);
    central.writeUInt16LE(nameBytes.length, 28);
    central.writeUInt32LE(name.endsWith('/') ? 0x10 : 0x20, 38);
    central.writeUInt32LE(offset, 42);
    locals.push(local, nameBytes, body);
    centrals.push(central, nameBytes);
    offset += local.length + nameBytes.length + body.length;
  }
  const directory = Buffer.concat(centrals);
  const end = Buffer.alloc(22);
  end.writeUInt32LE(0x06054b50, 0);
  end.writeUInt16LE(entries.length, 8);
  end.writeUInt16LE(entries.length, 10);
  end.writeUInt32LE(directory.length, 12);
  end.writeUInt32LE(offset, 16);
  return Buffer.concat([...locals, directory, end]);
}

const top = 'zenkai-v0.2.0-windows-x64/';
const release = zip([
  [top, null],
  [`${top}zenkai.exe`, 'launcher'],
  [`${top}zenkai-mcp.exe`, 'relay'],
  [`${top}LICENSE`, 'license'],
  [`${top}README.md`, 'readme'],
  [`${top}app/`, null],
  [`${top}app/zenkai.exe`, 'app 0.2.0'],
  [`${top}app/zenkai-pdf-host.exe`, 'pdf host'],
  [`${top}app/zenkai-values-host.exe`, 'values host'],
  [`${top}app/pdfium.dll`, 'pdfium'],
  [`${top}app/pdfium-LICENSE.txt`, 'pdfium license'],
]);
const hostile = zip([
  [`${top}app/zenkai.exe`, 'app 0.2.0'],
  [`${top}app/../../escaped.txt`, 'outside'],
]);

const current = testKey('current');
const next = testKey('next');
const stranger = testKey('stranger');
const comment = 'zenkai/0.2.0/windows-x64';

const files = {
  'zenkai-v0.2.0-windows-x64.zip': release,
  'good.minisig': minisig(release, current, comment),
  'next-key.minisig': minisig(release, next, comment),
  'old-version.minisig': minisig(release, current, 'zenkai/0.1.0/windows-x64'),
  'other-platform.minisig': minisig(release, current, 'zenkai/0.2.0/linux-x64'),
  'extra-field.minisig': minisig(release, current, 'zenkai/0.2.0/windows-x64/extra'),
  'default-comment.minisig': minisig(release, current, 'timestamp:1763700000\tfile:zenkai-v0.2.0-windows-x64.zip'),
  'wrong-key.minisig': minisig(release, stranger, comment),
  'forged-key-id.minisig': minisig(release, stranger, comment, { keyId: current.id }),
  'legacy.minisig': minisig(release, current, comment, { legacy: true }),
  'hostile.zip': hostile,
  'hostile.minisig': minisig(hostile, current, comment),
};
for (const [name, bytes] of Object.entries(files)) writeFileSync(join(here, name), bytes);
console.log(`TEST_CURRENT_KEY = ${current.publicKey}`);
console.log(`TEST_NEXT_KEY = ${next.publicKey}`);
