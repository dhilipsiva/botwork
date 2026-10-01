// The Node side of Botwork's JavaScript statements (decision D7). Botwork runs
// it as `node --input-type=module -e <this file> -- MODE FILE [INDEX]`:
//   list FILE        print the module's statement headers as a JSON array
//   call FILE INDEX  read one request (worker protocol version 1) on stdin,
//                    call statement INDEX, and write one response on stdout
// A module declares statements as an object mapping headers to functions:
//   export const statements = { "Greet |name|": (name) => `Hello, ${name}` };
import { pathToFileURL } from "node:url";

const [mode, file, index] = process.argv.slice(1);
const TRACE_BYTES = 2048;

async function statements() {
  const module = await import(pathToFileURL(file).href);
  const table = module.statements ?? module.default?.statements;
  if (table === null || typeof table !== "object") {
    throw new TypeError("the module exports no `statements` object of headers and functions");
  }
  for (const [header, implementation] of Object.entries(table)) {
    if (typeof implementation !== "function") {
      throw new TypeError(`the statement ${JSON.stringify(header)} is not a function`);
    }
  }
  return Object.entries(table);
}

// --- Worker protocol version 1: little-endian, length-prefixed. ---

class Writer {
  constructor() {
    this.parts = [];
  }
  bytes(buffer) {
    this.parts.push(buffer);
  }
  byte(value) {
    this.bytes(Buffer.from([value]));
  }
  u16(value) {
    const buffer = Buffer.alloc(2);
    buffer.writeUInt16LE(value);
    this.bytes(buffer);
  }
  u64(value) {
    const buffer = Buffer.alloc(8);
    buffer.writeBigUInt64LE(BigInt(value));
    this.bytes(buffer);
  }
  string(text) {
    const encoded = Buffer.from(text, "utf8");
    this.u64(encoded.length);
    this.bytes(encoded);
  }
  frame(kind) {
    const body = Buffer.concat(this.parts);
    const header = Buffer.alloc(15);
    header.write("BWIP", 0, "ascii");
    header.writeUInt16LE(1, 4);
    header.writeUInt8(kind, 6);
    header.writeBigUInt64LE(BigInt(body.length), 7);
    return Buffer.concat([header, body]);
  }
}

class Reader {
  constructor(buffer) {
    this.buffer = buffer;
    this.offset = 0;
  }
  take(count) {
    if (this.offset + count > this.buffer.length) {
      throw new Error("truncated request");
    }
    const slice = this.buffer.subarray(this.offset, this.offset + count);
    this.offset += count;
    return slice;
  }
  byte() {
    return this.take(1)[0];
  }
  u64() {
    const value = this.take(8).readBigUInt64LE();
    if (value > BigInt(Number.MAX_SAFE_INTEGER)) {
      throw new Error("oversized count");
    }
    return Number(value);
  }
  string() {
    return this.take(this.u64()).toString("utf8");
  }
  value() {
    const tag = this.byte();
    switch (tag) {
      case 0:
        return null;
      case 1:
        return this.take(4).readInt32LE();
      case 2:
        return this.take(4).readFloatLE();
      case 3:
        return this.byte() === 1;
      case 4:
        return this.string();
      case 5: {
        const items = [];
        for (let count = this.u64(); count > 0; count--) items.push(this.value());
        return items;
      }
      case 6: {
        const map = Object.create(null);
        for (let count = this.u64(); count > 0; count--) {
          const key = this.string();
          map[key] = this.value();
        }
        return map;
      }
      default:
        throw new Error(`unknown value tag ${tag}`);
    }
  }
}

function request(buffer) {
  const reader = new Reader(buffer);
  if (reader.take(4).toString("ascii") !== "BWIP" || reader.take(2).readUInt16LE() !== 1) {
    throw new Error("not a version 1 request");
  }
  if (reader.byte() !== 0) throw new Error("not a request");
  reader.u64();
  const values = [];
  for (let count = reader.u64(); count > 0; count--) values.push(reader.value());
  return values;
}

/** Why `value` has no Botwork equivalent, or null when it has one. */
function describe(value) {
  if (typeof value === "bigint") return `the BigInt ${value}`;
  if (typeof value === "number") return `the number ${value}`;
  if (typeof value === "function") return "a function";
  if (typeof value === "symbol") return "a symbol";
  const kind = value?.constructor?.name ?? "object";
  return `a ${kind}`;
}

const F32_MAX = 3.4028234663852886e38;

function encode(writer, value, depth = 0) {
  if (depth > 64) throw new RangeError("a value nested more than 64 levels deep");
  if (value === null || value === undefined) return writer.byte(0);
  switch (typeof value) {
    case "boolean":
      writer.byte(3);
      return writer.byte(value ? 1 : 0);
    case "string":
      writer.byte(4);
      return writer.string(value);
    case "number": {
      if (Number.isInteger(value) && value >= -2147483648 && value <= 2147483647) {
        writer.byte(1);
        const buffer = Buffer.alloc(4);
        buffer.writeInt32LE(value);
        return writer.bytes(buffer);
      }
      const narrow = Math.fround(value);
      if (!Number.isFinite(value) || !Number.isFinite(narrow) || Math.abs(value) > F32_MAX) {
        throw new RangeError(`${describe(value)}, which no finite 32-bit float holds`);
      }
      writer.byte(2);
      const buffer = Buffer.alloc(4);
      buffer.writeFloatLE(narrow);
      return writer.bytes(buffer);
    }
    case "object": {
      if (Array.isArray(value)) {
        writer.byte(5);
        writer.u64(value.length);
        for (const item of value) encode(writer, item, depth + 1);
        return;
      }
      const prototype = Object.getPrototypeOf(value);
      if (prototype === Object.prototype || prototype === null) {
        // Keys go in UTF-8 byte order, which the protocol requires.
        const keys = Object.keys(value).sort((left, right) =>
          Buffer.compare(Buffer.from(left, "utf8"), Buffer.from(right, "utf8")),
        );
        writer.byte(6);
        writer.u64(keys.length);
        for (const key of keys) {
          writer.string(key);
          encode(writer, value[key], depth + 1);
        }
        return;
      }
    }
  }
  throw new TypeError(`${describe(value)}, which has no Botwork equivalent`);
}

/** A diagnostic response: BW9001 for an assertion, BW4002 for anything else. */
function failure(code, text) {
  const writer = new Writer();
  writer.u64(0); // sources
  writer.u64(1); // errors
  writer.u16(code);
  writer.string(text);
  writer.u64(0); // the root node's error
  writer.string("source");
  writer.byte(0); // span
  writer.u64(0); // call frames
  writer.u64(0); // related locations
  writer.byte(0); // omissions
  writer.u64(0); // causes
  return writer.frame(2);
}

function explain(error) {
  if (!(error instanceof Error)) return `JavaScript threw ${describe(error)}: ${String(error)}`;
  let text = `JavaScript ${error.name}: ${error.message}`;
  const stack = (error.stack ?? "").split("\n").slice(1).join("\n");
  if (stack) {
    const encoded = Buffer.from(stack, "utf8");
    const tail = encoded.length > TRACE_BYTES ? "…\n" + encoded.subarray(-TRACE_BYTES).toString("utf8") : stack;
    text += "\n" + tail;
  }
  return text;
}

async function input() {
  const chunks = [];
  for await (const chunk of process.stdin) chunks.push(chunk);
  return Buffer.concat(chunks);
}

async function call() {
  const table = await statements();
  const [, implementation] = table[Number(index)];
  const values = request(await input());
  let response;
  try {
    const result = await implementation(...values);
    const writer = new Writer();
    try {
      encode(writer, result);
      response = writer.frame(1);
    } catch (error) {
      response = failure(4002, `The JavaScript statement returned ${error.message}`);
    }
  } catch (error) {
    response = error?.name === "AssertionError"
      ? failure(9001, error.message || "a JavaScript assertion failed")
      : failure(4002, explain(error));
  }
  process.stdout.write(response);
}

if (mode === "list") {
  const table = await statements();
  process.stdout.write(JSON.stringify(table.map(([header, implementation]) => [header, implementation.length])));
} else if (mode === "call") {
  await call();
} else {
  throw new Error(`unknown mode ${mode}`);
}
