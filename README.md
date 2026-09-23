# Pory

**Translate text right in your terminal — key-free out of the box, and any AI you plug in.
No browser, no copy-paste.**

`pory` is a small command-line translation tool with optional AI dictionary lookup. It works
out of the box, remembers translated content, and tells you honestly which backend produced
each result.

[English](README.md) · [简体中文](README.zh-CN.md) · [日本語](README.ja.md) · [Issues](https://github.com/harukizmoe/pory/issues)

```console
$ pory "今天天气不错"
The weather is nice today
◈ zh-CN ⇄ en · msedge · 1 block · cache 0/1 · 0.5s
```

The `◈` status line goes to **stderr**; redirecting stdout gives you only the translation.

## Install

Requires Rust and Cargo.

```bash
cargo install --git https://github.com/harukizmoe/pory --locked
```

Or build from source:

```bash
git clone https://github.com/harukizmoe/pory
cd pory
cargo build --release
```

## Quick start

```bash
pory "今天天气不错"            # -> The weather is nice today
pory "hello world"             # -> 你好，世界 (auto: English in, Chinese out)
pory "Good morning" -t ja      # -> おはようございます
pory "你好，世界" -t en         # -> Hello, world.
cat notes.md | pory -t en      # read from stdin
pory -b traditional "hello"    # use machine translation
pory "hello" --plain           # translation only
pory dict hello                  # AI-generated dictionary entry
pory dict "break the ice"        # phrase or idiom lookup
```

By default, `primary = "zh"` and `secondary = "en"`, so text is translated in either
direction without flags. Change these values to use another language pair.

## Features

- **No API key required** — works without a config file using the built-in translation
  chain. AI providers are optional; failed backends fall through to the next configured one.
- **Bring your provider** — supports key-free translation services and OpenAI-compatible
  APIs, with multiple models per provider.
- **Translate both ways** — set `primary` and `secondary`; auto mode chooses the direction
  from the detected source language.
- **AI dictionary lookup** — query English, Japanese, or Chinese words with pronunciation,
  parts of speech, senses, and examples when available. Entries are AI-generated, not authoritative.
- **Works in pipelines** — translation goes to stdout, status to stderr. Use `--plain` to
  suppress terminal decoration.
- **Chunking and cache** — long input is split at paragraph/sentence boundaries where
  possible, and completed chunks can be reused.
- **Shell helpers** — optional aliases and completions for fish, bash, and zsh; `--uninstall`
  removes the integration pory added.
- **Rust + rustls** — no OpenSSL dependency.

## Usage

```
Usage: pory [OPTIONS] [TEXT] [COMMAND]
```

| Option or command | Description |
|---|---|
| `[TEXT]` | Text to translate. Omitted → read from stdin (pipes supported) |
| `dict <TERM>` | Look up a word or phrase; quote multiword terms. |
| `-t, --target <LANG>` | Target language, e.g. `zh` / `en` / `ja` |
| `-f, --from <LANG>` | Source language, defaults to auto detection |
| `-b, --backend <MODE>` | `ai` / `traditional`. See below — it has two meanings |
| `--plain` | Print the translation only: no color, no footer, no animation |
| `--no-cache` | Don't read or write translation or dictionary cache entries |
| `--refresh` | Ignore existing cache entries and regenerate the result |
| `--timeout <SECONDS>` | Overall operation limit; defaults to 300 seconds (range: 1–86400) |
| `--clear-cache` | Clear translation and dictionary cache entries, then exit |
| `--init` | Write a sample config file and exit (never overwrites an existing one) |
| `--init-shell <SHELL>` | Install shell integration: `fish` / `bash` / `zsh` |
| `--print` | With `--init-shell`: print the script instead of installing it |
| `--uninstall` | Remove everything pory added to this system, then exit |

### Dictionary lookup

Use `pory dict hello` or `pory dict "break the ice"`. The result follows the configured
`primary` / `secondary` language direction; `-f` and `-t` can override it. The headword line
shows the primary part of speech beside the concise direct translation, after the headword and
pronunciation and before senses and examples. If AI lookup fails, Pory falls back to ordinary
translation and labels the result as translation only. This command does not accept `-b` or
`--plain`. Use `--no-cache` or `--refresh` to control cached entries.

### Backend selection

```bash
pory -b traditional "text"  # use it for this translation
pory -b traditional          # save it as the default mode
```

Without text, `-b` updates only the `mode` setting. Piped input counts as text.

### Supported languages

`zh` `zh-cn` `zh-tw` `en` `ja` `ko` `fr` `de` `es` `ru` `it` `pt` `ar` `th` `vi`

### Shell integration

```bash
pory --init-shell fish     # -> ~/.config/fish/completions/pory.fish
pory --init-shell bash     # -> completion file + `alias pr=pory` appended to ~/.bashrc
pory --init-shell zsh      # -> prints the script + appends the alias to ~/.zshrc
```

This adds the `pr` alias and tab completion. Changes are marked and reversible; files not
created by pory are not overwritten. Note: `pr` is also a POSIX command (`command pr` runs
it); zsh prints its completion script because the install location depends on `$fpath`.
Fish and bash were tested; zsh completion has not been tested in a real zsh shell.

### Uninstall

```bash
pory --uninstall
```

Removes pory's shell integration and cache. It asks before deleting your config (which may
contain an API key), and leaves the binary for Cargo or your package manager to remove.

## Configuration

Everything is optional. Run `pory --init` to write
`~/.config/pory/config.toml` (on Windows: `%APPDATA%\pory\`):

```toml
mode = "ai"                 # ai | traditional
primary = "zh"              # your language: the target when -t is not given
secondary = "en"            # the other language; leave "" to disable swapping
source = "auto"
cache = true                # false disables translation and dictionary caches

# ── AI providers (OpenAI-compatible; several can be configured) ──
[ai]
order = ["zhipu", "siliconflow"]   # tried in this order; providers without a key are skipped

[ai.siliconflow]
base_url = "https://api.siliconflow.cn/v1"
api_key = ""
models = ["tencent/Hunyuan-MT-7B", "Qwen/Qwen3-8B"]
thinking = false            # default off; set true to enable thinking for this provider

# ── Traditional backends (no key, always available) ──
# order decides both the chain used by mode = "traditional" and the safety net
# appended after the AI chain in mode = "ai".
[traditional]
order = ["msedge", "transmart", "mymemory", "google"]
```

If the file is missing, built-in defaults are used — that is not an error. The same goes
for the cache: if it is unreadable or corrupted, pory silently falls back to an empty one
rather than refusing to work.
On Unix, `pory --init` creates the config file readable and writable only by its owner.
For a manually created or older config, use `chmod 600 ~/.config/pory/config.toml` to protect API keys.

### Language direction

In auto mode, pory translates between `primary` and `secondary` based on the detected
source language:

```bash
pory "hello world"     # source is English → translate into 中文
pory "你好，世界"       # source is already 中文 → translate into English
```

An explicit `-t` or `-f` disables direction swapping; set `secondary = ""` to disable it.

## Backends

The built-in key-free backends are `msedge`, `transmart`, `mymemory`, and `google`. AI
providers use OpenAI-compatible APIs; set `base_url` and `models` under `[ai.<name>]`.
Examples include DeepSeek, Kimi, Zhipu, SiliconFlow, and Ollama. Pory tries multiple models
in order before moving to the next provider.

Each backend request has a **30-second timeout**. One translation has a **300-second overall
limit** shared by every chunk and fallback attempt. When it expires, pory reports an error
and keeps the cache for completed chunks. Use `--timeout 900` for a 15-minute limit
(maximum 86400 seconds). A single backend timeout still moves on to the next fallback.

## Contributing

Issues and pull requests are welcome. A few things that make a patch easy to accept:

- `cargo test` and `cargo clippy --all-targets` should both be clean
- Comments in the source are in Chinese; `--help` text is English only (it is the user
  interface, and the project deliberately keeps it to one language)
- New features must be **opt-in**: with no configuration, the code path and behavior must
  be identical to before the feature existed

## License

[MIT](LICENSE)
