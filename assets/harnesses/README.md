# Harness logos

Official artwork downloaded on 2026-10-22, used to identify supported integrations.
The logos and trademarks belong to their respective owners and are separate from actX's MIT license.

| Integration | Original artwork | Source |
| --- | --- | --- |
| Claude | `claude.png` | [Claude website favicon](https://assets.claude.com/95a868946ac8a31e5ff832e2899f294aa368b836.png?w=32&h=32), linked by [claude.com](https://claude.com/) |
| Codex | `codex.svg` (OpenAI white Blossom) | [OpenAI logo download](https://cdn.openai.com/brand/OpenAI-Logos-2025.zip), [brand guidelines](https://openai.com/brand/) |
| opencode | `opencode.svg` | [Anomaly's models.dev repository](https://github.com/anomalyco/models.dev/blob/dev/providers/opencode/logo.svg), [brand page](https://opencode.ai/brand) |
| Antigravity | `antigravity.svg` (white icon) | [Official press assets](https://www.antigravity.google/press), [SVG](https://www.antigravity.google/assets/image/brand/antigravity-icon__white.svg) |

The `.rgba` files are 64 × 64, unpremultiplied RGBA8 renditions of the originals,
embedded in the executable. SVGs were rendered with librsvg (`rsvg-convert`), using
white for `currentColor`; raster resizing used Lanczos. Aspect ratios are preserved.
Textures load once per GUI context. No network request or SVG parser runs at startup.
