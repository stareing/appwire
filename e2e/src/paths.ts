import { dirname, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

export const E2E_ROOT = resolve(dirname(fileURLToPath(import.meta.url)), '..')
export const REPO_ROOT = resolve(E2E_ROOT, '..')
export const SHOP_DIR = resolve(REPO_ROOT, 'examples/shop')
export const SHOP_MANIFEST = resolve(SHOP_DIR, 'app-mcp.json')
