import { readFile } from 'node:fs/promises';
import { join } from 'node:path';
import { AssetLibrary, type AssetSource } from '../src/core/assets';

/** Asset library reading from public/assets on disk (for CLI tools and tests). */
export function nodeLibrary(dir = join(import.meta.dirname, '../public/assets')): Promise<AssetLibrary> {
  const source: AssetSource = { read: async (p) => new Uint8Array(await readFile(join(dir, p))) };
  return new AssetLibrary(source).init();
}
