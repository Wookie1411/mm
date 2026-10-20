/**
 * Just enough of a ZIP reader to open launcher modpacks in the browser: the central
 * directory, stored and deflated entries (via the native DecompressionStream). No ZIP64,
 * no encryption; modpacks use neither.
 */

export interface ZipEntry {
  name: string;
  method: number;
  compressedSize: number;
  size: number;
  offset: number;
}

const u16 = (v: DataView, o: number) => v.getUint16(o, true);
const u32 = (v: DataView, o: number) => v.getUint32(o, true);
const decoder = new TextDecoder();

const bytes = async (file: Blob, start: number, end: number) =>
  new Uint8Array(await file.slice(start, end).arrayBuffer());

export async function listZip(file: Blob): Promise<ZipEntry[]> {
  // end of central directory: 22 bytes + up to 64 KiB comment
  const tailStart = Math.max(0, file.size - 22 - 0xffff);
  const tail = await bytes(file, tailStart, file.size);
  const tv = new DataView(tail.buffer);
  let eocd = -1;
  for (let i = tail.length - 22; i >= 0; i--) {
    if (u32(tv, i) === 0x06054b50) {
      eocd = i;
      break;
    }
  }
  if (eocd < 0) throw new Error('not a zip file');
  const count = u16(tv, eocd + 10);
  const cdSize = u32(tv, eocd + 12);
  const cdOffset = u32(tv, eocd + 16);
  if (count === 0xffff || cdOffset === 0xffffffff) throw new Error('ZIP64 archives are not supported');

  const cd = await bytes(file, cdOffset, cdOffset + cdSize);
  const v = new DataView(cd.buffer);
  const entries: ZipEntry[] = [];
  let p = 0;
  for (let i = 0; i < count && p + 46 <= cd.length; i++) {
    if (u32(v, p) !== 0x02014b50) throw new Error('corrupt zip directory');
    const nameLen = u16(v, p + 28);
    const extraLen = u16(v, p + 30);
    const commentLen = u16(v, p + 32);
    entries.push({
      method: u16(v, p + 10),
      compressedSize: u32(v, p + 20),
      size: u32(v, p + 24),
      offset: u32(v, p + 42),
      name: decoder.decode(cd.subarray(p + 46, p + 46 + nameLen)),
    });
    p += 46 + nameLen + extraLen + commentLen;
  }
  return entries;
}

export async function readEntry(file: Blob, entry: ZipEntry): Promise<Uint8Array> {
  const header = new DataView((await bytes(file, entry.offset, entry.offset + 30)).buffer);
  if (u32(header, 0) !== 0x04034b50) throw new Error(`corrupt zip entry ${entry.name}`);
  const start = entry.offset + 30 + u16(header, 26) + u16(header, 28);
  const raw = file.slice(start, start + entry.compressedSize);
  if (entry.method === 0) return new Uint8Array(await raw.arrayBuffer());
  if (entry.method !== 8) throw new Error(`unsupported compression in ${entry.name}`);
  const stream = raw.stream().pipeThrough(new DecompressionStream('deflate-raw'));
  return new Uint8Array(await new Response(stream).arrayBuffer());
}

export async function sha1Hex(data: Uint8Array): Promise<string> {
  const digest = await crypto.subtle.digest('SHA-1', data as BufferSource);
  return [...new Uint8Array(digest)].map((b) => b.toString(16).padStart(2, '0')).join('');
}

export interface PackContents {
  kind: 'mrpack' | 'curseforge';
  index: unknown;
  overrides: { path: string; sha1: string }[];
}

/** Open a Modrinth (.mrpack) or CurseForge (.zip) modpack export. */
export async function readPack(file: Blob): Promise<PackContents> {
  const entries = await listZip(file);
  const find = (name: string) => entries.find((e) => e.name === name);
  const json = async (e: ZipEntry) => JSON.parse(decoder.decode(await readEntry(file, e)));

  let kind: PackContents['kind'];
  let index: unknown;
  let roots: string[];
  const mrIndex = find('modrinth.index.json');
  const cfManifest = find('manifest.json');
  if (mrIndex) {
    kind = 'mrpack';
    index = await json(mrIndex);
    // client-overrides are client-only by definition
    roots = ['overrides/', 'server-overrides/'];
  } else if (cfManifest) {
    kind = 'curseforge';
    index = await json(cfManifest);
    const dir = (index as { overrides?: unknown }).overrides;
    roots = [`${typeof dir === 'string' && dir ? dir.replace(/\/+$/, '') : 'overrides'}/`];
  } else {
    throw new Error('no modrinth.index.json or manifest.json in the archive');
  }

  const jars = entries.filter(
    (e) =>
      e.name.toLowerCase().endsWith('.jar') &&
      roots.some((r) => e.name.startsWith(`${r}mods/`) || e.name.startsWith(`${r}plugins/`)),
  );
  const overrides = [];
  for (const e of jars) {
    overrides.push({ path: e.name, sha1: await sha1Hex(await readEntry(file, e)) });
  }
  return { kind, index, overrides };
}
