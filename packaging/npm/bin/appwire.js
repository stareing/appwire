#!/usr/bin/env node
// @ts-check
import { main } from '../lib/launcher.js'

main(import.meta.url, process.argv.slice(2))
