import { describe, expect, it } from 'vitest'
import { existsSync } from 'node:fs'

describe('scaffold', () => {
  it('has native runtime entry points and bundled fonts', () => {
    expect(existsSync('apps/core/src/main.rs')).toBe(true)
    expect(existsSync('apps/core/src/windows_overlay/mod.rs')).toBe(true)
    expect(existsSync('apps/assets/fonts/Inter/ttf/Inter-Regular.ttf')).toBe(true)
    expect(existsSync('apps/assets/fonts/Inter/ttf/Inter-Bold.ttf')).toBe(true)
  })
})
