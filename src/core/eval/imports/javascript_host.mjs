// The Node side of Botwork's JavaScript statements (decision D7). Botwork runs
// it as `node --input-type=module -e <this file> -- MODE FILE [INDEX]`:
//   list FILE        print the module's statement headers as a JSON array
//   call FILE INDEX  read one request (worker protocol version 1) on stdin,
//                    call statement INDEX, and write one response on stdout
// A module declares statements as an object mapping headers to functions:
//   export const statements = { "Greet |name|": (name) => `Hello, ${name}` };
import { readFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const [mode, file, index] = process.argv.slice(1);
const TRACE_BYTES = 2048;
// The most causes a failure keeps of its chain, and the most of its file a
// "raised here" location holds, as for Python. A response's locations hold at
// most RAISED_TOTAL bytes of files, well within a worker frame.
const CAUSES = 8;
const RAISED_BYTES = 256 * 1024;
const RAISED_TOTAL = 512 * 1024;

// Stdout carries the response alone. What a module prints, to process.stdout
// or through the console, which writes there, goes to stderr instead, which
// Botwork bounds and discards.
const respond = process.stdout.write.bind(process.stdout);
process.stdout.write = process.stderr.write.bind(process.stderr);

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

/** Whether `text` has no unpaired surrogate, which UTF-8 cannot hold. */
function wellFormed(text) {
  return typeof text.isWellFormed === "function"
    ? text.isWellFormed()
    : !/[\uD800-\uDBFF](?![\uDC00-\uDFFF])|(?<![\uD800-\uDBFF])[\uDC00-\uDFFF]/.test(text);
}

function checkText(text) {
  if (!wellFormed(text)) {
    throw new TypeError("a string with an unpaired surrogate, which UTF-8 cannot hold");
  }
}

function encode(writer, value, depth = 0) {
  if (depth > 64) throw new RangeError("a value nested more than 64 levels deep");
  if (value === null || value === undefined) return writer.byte(0);
  switch (typeof value) {
    case "boolean":
      writer.byte(3);
      return writer.byte(value ? 1 : 0);
    case "string":
      checkText(value);
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
      // A whole number beyond 32-bit integers becomes a float only if one
      // holds it exactly; rounding it would change it silently.
      if (Number.isInteger(value) && narrow !== value) {
        throw new RangeError(`${describe(value)}, a whole number beyond 32 bits that no 32-bit float holds exactly`);
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
          checkText(key);
          writer.string(key);
          encode(writer, value[key], depth + 1);
        }
        return;
      }
    }
  }
  throw new TypeError(`${describe(value)}, which has no Botwork equivalent`);
}

/**
 * Where `error` was raised: the innermost frame of its stack in a file beside
 * the module, as that file up to the line and the line's byte range in it.
 */
function raised(error) {
  const stack = typeof error?.stack === "string" ? error.stack : "";
  const directory = path.dirname(file) + path.sep;
  for (const frame of stack.split("\n").filter((line) => /^\s+at /.test(line))) {
    const match = /(file:\/\/[^\s()]+):(\d+):\d+/.exec(frame);
    if (!match) continue;
    let source;
    try {
      source = fileURLToPath(match[1]);
    } catch {
      continue;
    }
    // Frames of this host, which Node names `[eval1]`, are not the module's.
    if (!source.startsWith(directory) || /\[eval\d*\]$/.test(source)) continue;
    let text;
    try {
      text = readFileSync(source, "utf8");
    } catch {
      return undefined;
    }
    const lines = text.split("\n");
    const line = Number(match[2]);
    if (line < 1 || line > lines.length) return undefined;
    const before = Buffer.byteLength(lines.slice(0, line - 1).map((each) => `${each}\n`).join(""));
    const shown = lines[line - 1];
    const end = before + Buffer.byteLength(shown);
    if (end > RAISED_BYTES) return undefined;
    const first = before + Buffer.byteLength(shown) - Buffer.byteLength(shown.trimStart());
    const last = Math.max(first, before + Buffer.byteLength(shown.trimEnd()));
    return { file: source, text: Buffer.from(text, "utf8").subarray(0, end).toString("utf8"), first, last };
  }
  return undefined;
}

/** One failure: BW9001 for an assertion, BW4002 for anything else. */
function one(error) {
  const assertion = error?.name === "AssertionError";
  return {
    code: assertion ? 9001 : 4002,
    text: assertion ? error.message || "a JavaScript assertion failed" : explain(error),
    raised: raised(error),
  };
}

/** `error` and the errors it chains through `cause`, outermost first. */
function chain(error) {
  const failures = [one(error)];
  const seen = new Set([error]);
  let current = error;
  while (
    failures.length <= CAUSES &&
    current !== null &&
    typeof current === "object" &&
    current.cause !== undefined &&
    !seen.has(current.cause)
  ) {
    current = current.cause;
    seen.add(current);
    failures.push(one(current));
  }
  return failures;
}

/**
 * A diagnostic response for `failures`, each the cause of the one before it,
 * with where each was raised.
 */
function failure(failures) {
  // One source per file, its text up to the furthest line a failure names;
  // a location that would take the sources past RAISED_TOTAL is left out.
  const sources = new Map();
  let total = 0;
  for (const each of failures) {
    const { raised } = each;
    if (!raised) continue;
    const known = sources.get(raised.file) ?? "";
    const grows = Math.max(0, Buffer.byteLength(raised.text) - Buffer.byteLength(known));
    if (total + grows > RAISED_TOTAL) {
      each.raised = undefined;
      continue;
    }
    total += grows;
    if (grows > 0) sources.set(raised.file, raised.text);
  }
  const files = [...sources.keys()];
  const writer = new Writer();
  writer.u64(files.length);
  for (const name of files) {
    writer.string(name);
    writer.string(sources.get(name));
  }
  writer.u64(failures.length); // errors
  for (const { code, text } of failures) {
    writer.u16(code);
    writer.string(text);
  }
  failures.forEach(({ raised }, index) => {
    writer.u64(index); // the node's error
    writer.string("source");
    writer.byte(0); // span
    writer.u64(0); // call frames
    if (raised) {
      writer.u64(1); // related locations
      writer.string("raised here");
      writer.u64(files.indexOf(raised.file));
      writer.u64(raised.first);
      writer.u64(raised.last);
    } else {
      writer.u64(0);
    }
    writer.byte(0); // omissions
    writer.u64(index + 1 < failures.length ? 1 : 0); // causes
  });
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
      response = failure([{ code: 4002, text: `The JavaScript statement returned ${error.message}` }]);
    }
  } catch (error) {
    response = failure(chain(error));
  }
  respond(response);
}

if (mode === "list") {
  const table = await statements();
  respond(JSON.stringify(table.map(([header, implementation]) => [header, implementation.length])));
} else if (mode === "call") {
  await call();
} else {
  throw new Error(`unknown mode ${mode}`);
}
