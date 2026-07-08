import type { BinaryRangeFrame } from './types';

const RANGE_MAGIC = 'BDSR';
const RANGE_VERSION = 1;
const RANGE_HEADER_BYTES = 10;

function magicString(bytes: Uint8Array): string {
  return String.fromCharCode(...bytes.slice(0, 4));
}

export function parseBinaryRangeFrame(bytes: Uint8Array): BinaryRangeFrame {
  if (bytes.byteLength < RANGE_HEADER_BYTES) {
    throw new Error(`range frame too short: ${bytes.byteLength} bytes`);
  }
  if (magicString(bytes) !== RANGE_MAGIC) {
    throw new Error('bad range magic');
  }

  const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
  const version = view.getUint16(4, true);
  if (version !== RANGE_VERSION) {
    throw new Error(`unsupported range version: ${version}`);
  }

  const count = view.getUint32(6, true);
  const messages: Uint8Array[] = [];
  let pos = RANGE_HEADER_BYTES;

  for (let i = 0; i < count; i += 1) {
    if (pos + 4 > bytes.byteLength) {
      throw new Error('truncated range item length');
    }
    const len = view.getUint32(pos, true);
    pos += 4;
    if (pos + len > bytes.byteLength) {
      throw new Error('truncated range item body');
    }
    messages.push(bytes.slice(pos, pos + len));
    pos += len;
  }

  if (pos !== bytes.byteLength) {
    throw new Error('trailing bytes after range frame');
  }

  return { version, messages };
}
