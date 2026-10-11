# Agent marks

One file per agent in this folder. Each mark is a single monochrome path that
takes the surrounding text colour. Custom agents keep the letter mark.

## codex.svg

- Source: `https://cdn.agentclientprotocol.com/registry/v1/latest/codex-acp.svg`
  (agent id `codex-acp` in
  `https://cdn.agentclientprotocol.com/registry/v1/latest/registry.json`),
  fetched 2026-10-10.
- Terms: the registry entry lists Apache-2.0 for the Codex adapter by OpenAI,
  JetBrains s.r.o. and Zed Industries
  (`https://github.com/agentclientprotocol/codex-acp/blob/main/LICENSE`).
  Used unmodified to identify the agent, the same way the ACP registry and Zed
  use it.
- Note: the file as published contains no scripts and no editor metadata, so
  nothing was stripped. It paints with the surrounding text colour
  (`fill="currentColor"`), which is why the tile renders it as-is.

## opencode.svg

- Source: `packages/console/app/src/asset/brand/opencode-logo-dark-square.svg`
  in https://github.com/sst/opencode (dev branch).
- Terms: MIT License, Copyright (c) 2025 opencode. Adapted: the frame and the
  block merged into one path, and `fill="currentColor"` added to the path so it
  takes the tile colour. That is a colour attribute only; the artwork is unchanged.
- Note: this is the mark of sst/opencode, the terminal agent Zenkai starts. It
  is not the logo of the newer OpenCode v2 product at opencode.ai/brand.

## Skipped brands

- Gemini: stays on the letter mark (deprecated). Google retired Gemini CLI for free and
  individual accounts on 2026-06-18; its successor is Antigravity CLI. No
  official standalone SVG was pursued.
