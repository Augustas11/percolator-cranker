export type Integer = number | bigint;

function checked(value: Integer, bits: number, signed = false): bigint {
  if (typeof value === "number" && !Number.isSafeInteger(value)) {
    throw new RangeError("Integer must be a safe integer number or bigint");
  }
  const n = BigInt(value);
  const bound = 1n << BigInt(signed ? bits - 1 : bits);
  if (n < (signed ? -bound : 0n) || n >= bound) {
    throw new RangeError(`${signed ? "i" : "u"}${bits} out of range`);
  }
  return n;
}

export class Writer {
  private readonly bytes: number[] = [];

  private push(value: Integer, bits: number, signed = false): this {
    const n = BigInt.asUintN(bits, checked(value, bits, signed));
    const bytes = new Uint8Array(bits / 8);
    const view = new DataView(bytes.buffer);
    if (bits === 8) view.setUint8(0, Number(n));
    else if (bits === 16) view.setUint16(0, Number(n), true);
    else if (bits === 32) view.setUint32(0, Number(n), true);
    else {
      view.setBigUint64(0, n & ((1n << 64n) - 1n), true);
      if (bits === 128) view.setBigUint64(8, n >> 64n, true);
    }
    this.bytes.push(...bytes);
    return this;
  }

  u8(value: Integer): this { return this.push(value, 8); }
  u16(value: Integer): this { return this.push(value, 16); }
  u32(value: Integer): this { return this.push(value, 32); }
  u64(value: Integer): this { return this.push(value, 64); }
  u128(value: Integer): this { return this.push(value, 128); }
  i128(value: Integer): this { return this.push(value, 128, true); }
  finish(): Uint8Array { return Uint8Array.from(this.bytes); }
}

function view(data: Uint8Array): DataView {
  return new DataView(data.buffer, data.byteOffset, data.byteLength);
}

export function u8(data: Uint8Array, offset: number): number { return view(data).getUint8(offset); }
export function u16(data: Uint8Array, offset: number): number { return view(data).getUint16(offset, true); }
export function u32(data: Uint8Array, offset: number): number { return view(data).getUint32(offset, true); }
export function u64(data: Uint8Array, offset: number): bigint { return view(data).getBigUint64(offset, true); }
export function u128(data: Uint8Array, offset: number): bigint {
  const v = view(data);
  return v.getBigUint64(offset, true) | (v.getBigUint64(offset + 8, true) << 64n);
}
export function i128(data: Uint8Array, offset: number): bigint { return BigInt.asIntN(128, u128(data, offset)); }
