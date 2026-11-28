// Runs the official Readability implementation on a saved HTML snapshot.
import { openSync, closeSync, fstatSync, readSync, statSync } from "node:fs";
import { createRequire } from "node:module";
import { resolve } from "node:path";

const MAX_INPUT_BYTES = 2 * 1024 * 1024;
const [modules, snapshot] = process.argv.slice(2);
const requirePackage = createRequire(resolve(modules, "../package.json"));
const { Readability } = requirePackage("@mozilla/readability");
const { JSDOM, VirtualConsole } = requirePackage("jsdom");

function readSnapshot(path) {
  const metadata = statSync(path, { throwIfNoEntry: false });
  if (!metadata?.isFile() || metadata.size > MAX_INPUT_BYTES) return "";
  const descriptor = openSync(path, "r");
  try {
    if (!fstatSync(descriptor).isFile()) return "";
    const buffer = Buffer.alloc(MAX_INPUT_BYTES + 1);
    let length = 0;
    while (length < buffer.length) {
      const read = readSync(descriptor, buffer, length, buffer.length - length, null);
      if (read === 0) break;
      length += read;
    }
    if (length > MAX_INPUT_BYTES) return "";
    try {
      return new TextDecoder("utf-8", { fatal: true }).decode(buffer.subarray(0, length));
    } catch {
      return "";
    }
  } finally {
    closeSync(descriptor);
  }
}

const html = readSnapshot(snapshot);
const packages = {
  readability: requirePackage(resolve(modules, "@mozilla/readability/package.json")).version,
  jsdom: requirePackage(resolve(modules, "jsdom/package.json")).version,
};
let document;
let content = "";
let error = null;
try {
  // jsdom disables scripts and external subresources by default. Keep those defaults.
  document = new JSDOM(html, {
    url: "https://benchmark.invalid/",
    virtualConsole: new VirtualConsole(),
  });
  const article = new Readability(document.window.document).parse();
  content = article?.content ?? "";
} catch {
  error = "parse_failed";
} finally {
  document?.window.close();
}
process.stdout.write(JSON.stringify({ content, packages, error }));
