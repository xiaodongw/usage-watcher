# Where these came from

Each file is the vendor's own mark, taken from the vendor's own site on
2026-08-19, resized to 64×64 and palette-quantised (see
`docs/ADDING-A-PROVIDER.md` for the recipe). They are used to identify the
provider a row refers to, which is what a trademark is for; none of them
implies the vendor endorses this tool.

| file | source |
| --- | --- |
| `claude.png` | `https://claude.com/apple-touch-icon.png` |
| `codex.png` | `https://developers.openai.com/favicon.png` — the Codex CLI docs site. `openai.com` and `chatgpt.com` both refuse automated requests. |
| `opencode.png` | `https://opencode.ai/favicon-96x96-v3.png` |
| `openrouter.png` | `https://openrouter.ai/favicon/glyph.png` |

Re-download only when a vendor rebrands. Note that `claude.png` and
`opencode.png` carry their own opaque background and `codex.png` and
`openrouter.png` are transparent outside the mark; all four were checked
against both the light and the dark panel background, and the CSS does not
tint them.
