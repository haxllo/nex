# Project Charter

## Product Name

Working name: `Nex`.

## Problem Statement

Windows users who rely on keyboard workflows need a fast launcher that finds apps and files instantly. Windows built-in search is often noisy, slower for direct local intents, and less optimized for command-palette style usage.

## Vision

Build a fast local launcher for Windows: a lightweight floating fuzzy-finder that opens in under 60ms and returns high-quality local results in under 15ms for common queries.

## Primary Users

- Power users who launch apps and files via keyboard
- Developers and creators switching across tools frequently
- Laptop users who need low resource usage

## Goals

- Global hotkey to invoke and dismiss a floating search bar
- Unified local search for apps, files, folders, and commands
- Fuzzy matching with typo tolerance and smart ranking
- Modern UI with fully keyboard-driven interactions
- Configurable behavior without sacrificing speed

## Non-Goals (MVP)

- Web search integration
- AI assistant features
- Deep content search inside files
- Cloud sync

## Success Metrics

- P50 hotkey-to-visible latency: <= 60ms
- P95 query-to-results latency: <= 15ms for warm index
- Idle CPU usage: near 0% most of the time
- Idle memory target (combined): <= 120MB
- Launch success rate: >= 99.9%

## Constraints

- Must run on Windows 10/11 x64
- Must function fully offline
- Must be stable as an always-on background process
- Must degrade gracefully on low-end hardware
