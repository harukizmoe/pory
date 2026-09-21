# Pory

**Translate text right in your terminal — key-free out of the box, and any AI you plug in.
No browser, no copy-paste.**

`pory` is a small command-line translator for people who live in a shell. It works out of
the box, remembers what it has already translated, and tells you honestly which backend
actually produced the result.

> Named after [Porygon](https://en.wikipedia.org/wiki/Porygon) — a creature made of data
> that adapts to any environment. Translation is the same job: turning one symbol system
> into another.

[English](README.md) · [简体中文](README.zh-CN.md) · [日本語](README.ja.md)

```console
$ pory "今天天气不错"
The weather is nice today
◈ zh-CN ⇄ en · msedge · 1 block · cache 0/1 · 0.5s
```

That `◈` line is the only decoration. It says which languages, which backend, how many
chunks, how much came from cache, and how long the whole call took — and it goes to
**stderr**, so redirecting stdout gives you nothing but the translation.

## Features

- **Two modes, one key** — `ai` (a large model: best quality, understands context) or
  `traditional` (key-free machine translation: fastest). In `ai` mode the traditional
  chain is always appended as a safety net, so a missing key or a rate limit degrades
  instead of failing. `-b ai` / `-b traditional` switches per call.
- **Zero configuration** — with no config at all it still works: the traditional chain
  starts with `msedge`, which needs no API key and is reachable from mainland China.
- **Pluggable backends** — four key-free traditional backends, plus any OpenAI-compatible
  API (DeepSeek, Kimi, Zhipu, SiliconFlow, local Ollama…). One provider can carry several
  models; when the primary model fails, its siblings are tried before the next provider.
- **Automatic fallback, always visible** — if a backend is missing a key, out of quota or
  times out, the next one takes over. A warning says what happened and the footer shows the
  real path — silent fallback would leave you thinking you were using an LLM when you were
  not.
- **AI thinking switch** — `thinking = false` turns off the reasoning phase of hybrid
  models. Measured on SiliconFlow: Qwen3-8B went from 22.9 s to 1.4 s per sentence, with
  no quality loss.
- **Bidirectional by default** — set a primary and a secondary language. In auto mode, text
  already in your primary language is translated *into* your secondary one, instead of
  being handed back to you unchanged.
- **Shell integration** — `pory --init-shell fish` installs a short name (`pr` → `pory`)
  and tab completion; `bash` and `zsh` are supported too.
- **Clean uninstall** — `pory --uninstall` removes everything it added (completion files,
  the lines it appended to your rc files, the cache) and asks once before touching your
  config.
- **Pipe-friendly** — `cat notes.md | pory -t en`. Colors, the footer and the progress
  animation only appear when the stream is a real terminal, so pipes, CI and redirections
  stay byte-clean.
- **Smart chunking** — long text is split on paragraph → sentence → hard cut, so nothing
  gets chopped mid-word.
- **Chunk-level cache** — a sentence you translated last week is free today. Cache hits
  return in about 4 ms.
- **Small and fast** — a single static binary (~3.7 MB), ~3 ms startup, no runtime
  dependencies.
- **Pure Rust + rustls** — no OpenSSL, so cross-compiling to musl or ARM doesn't drag in
  system libraries.

## Install

```bash
cargo install --git https://github.com/harukizmoe/pory --locked
```

`--locked` pins the dependencies to the versions recorded in the repository's
`Cargo.lock`, so a later release of some dependency cannot break your install.

Or build from source:

```bash
git clone https://github.com/harukizmoe/pory
cd pory
cargo build --release        # binary at target/release/pory
```

## Quick start

```bash
pory "今天天气不错"            # -> The weather is nice today
pory "hello world"             # -> 你好，世界        (auto: English in, Chinese out)
pory "Good morning" -t ja      # -> おはようございます
pory "你好，世界" -t en         # -> Hello, world.
pory "こんにちは" -f ja -t zh   # explicit source and target
cat notes.md | pory -t en      # read from stdin
pory "hello" --plain           # translation only, no footer, no animation
pory -b traditional "hello"    # skip the AI entirely, use machine translation
```

By default `primary = "zh"` and `secondary = "en"`, so the first two examples work in both
directions with no flags. Change them in the config to make any pair work the same way.

## Usage

```
Usage: pory [OPTIONS] [TEXT]
```

| Option | Description |
|---|---|
| `[TEXT]` | Text to translate. Omitted → read from stdin (pipes supported) |
| `-t, --target <LANG>` | Target language, e.g. `zh` / `en` / `ja` |
| `-f, --from <LANG>` | Source language, defaults to auto detection |
| `-b, --backend <MODE>` | `ai` / `traditional`. See below — it has two meanings |
| `--plain` | Print the translation only: no color, no footer, no animation |
| `--no-cache` | Don't use the cache: neither read existing entries nor write new ones |
| `--refresh` | Ignore the existing cache, re-translate and update it |
| `--clear-cache` | Clear the translation cache and exit |
| `--init` | Write a sample config file and exit (never overwrites an existing one) |
| `--init-shell <SHELL>` | Install shell integration: `fish` / `bash` / `zsh` |
| `--print` | With `--init-shell`: print the script instead of installing it |
| `--uninstall` | Remove everything pory added to this system, then exit |

### `-b` has two meanings

Which one you get depends on whether there is any input:

```bash
pory -b traditional "text"           # use the traditional chain for this call
echo "text" | pory -b traditional    # same — piped input also counts as "this call"
pory -b traditional                  # no text → save it to the config as the new default
```

The last form prints `✓ 已把默认翻译模式设为 traditional` and exits, rewriting only the
`mode` line in your config file — comments and everything else stay untouched.

If you combine `-b` with another flag but no text (`pory -b ai -t ja`), pory prints a usage
hint instead of silently editing your config — that case is almost always a typo.

### Supported languages

`zh` `zh-cn` `zh-tw` `en` `ja` `ko` `fr` `de` `es` `ru` `it` `pt` `ar` `th` `vi`

### Shell integration

```bash
pory --init-shell fish     # -> ~/.config/fish/completions/pory.fish
pory --init-shell bash     # -> completion file + `alias pr=pory` appended to ~/.bashrc
pory --init-shell zsh      # -> prints the script + appends the alias to ~/.zshrc
```

What it gives you: a short name (`pr` + space expands to `pory`) and tab completion for
languages, modes and options. Every change is idempotent and marked, so `pory --uninstall`
can undo it, and a file that pory did not write is never overwritten.

Two details worth knowing:

- `pr` shares its name with the POSIX paginator `/usr/bin/pr`. In an interactive shell it
  expands to `pory`; type `\pr` or `command pr` to reach the real one.
- zsh has no standard location for completion files (it depends on your `$fpath`), so for
  zsh pory prints the script and tells you where to put it. The zsh completions are
  written to spec but untested on a real zsh — fish and bash were verified by running them.

### Uninstall

```bash
pory --uninstall
```

Removes the completion files, the lines pory appended to `~/.bashrc` / `~/.zshrc`, and the
translation cache — without asking, because all of that can be regenerated. Your config
file is a different matter: it may hold an API key and settings you tuned, so pory asks
once (`[y/N]`) before deleting it. In a non-interactive context it always keeps the config.

The binary itself is not removed (it may be managed by cargo or a package manager); the
command prints how to delete it.

## Configuration

Everything is optional. Run `pory --init` to write
`~/.config/pory/config.toml` (on Windows: `%APPDATA%\pory\`):

```toml
mode = "ai"                 # ai | traditional
primary = "zh"              # your language: the target when -t is not given
secondary = "en"            # the other language; leave "" to disable swapping
source = "auto"
cache = true                # false is equivalent to always passing --no-cache

# ── AI providers (OpenAI-compatible; several can be configured) ──
[ai]
order = ["zhipu", "siliconflow"]   # tried in this order; providers without a key are skipped

[ai.siliconflow]
base_url = "https://api.siliconflow.cn/v1"
api_key = ""
models = ["tencent/Hunyuan-MT-7B", "Qwen/Qwen3-8B"]
thinking = false            # optional: send enable_thinking = false

# ── Traditional backends (no key, always available) ──
# order decides both the chain used by mode = "traditional" and the safety net
# appended after the AI chain in mode = "ai".
[traditional]
order = ["msedge", "transmart", "mymemory", "google"]
```

If the file is missing, built-in defaults are used — that is not an error. The same goes
for the cache: if it is unreadable or corrupted, pory silently falls back to an empty one
rather than refusing to work.

### Primary / secondary swapping

In auto mode (neither `-f` nor `-t` given), pory picks the direction from the detected
source language:

```bash
pory "hello world"     # source is English → translate into 中文
pory "你好，世界"       # source is already 中文 → translate into English
```

So you never make a pointless "Chinese → Chinese" request. Three boundaries:

- An explicit `-t` never changes direction — you asked for a target, you get it
- An explicit `-f` doesn't swap either — the source is known, and if it equals the target
  pory returns the text locally without any request
- `secondary = ""` disables swapping entirely

This is not free magic — every backend has to handle it, and the mechanism differs by
protocol:

- **MyMemory** reports `translatedText: null` plus a detected language; pory issues one
  extra request against the secondary target.
- **AI backends** get both directions in a single prompt — the two languages are named
  `A` and `B` first, so the model cannot confuse the target with the condition. One round
  trip, no detection call.
- **msedge / transmart / google** just translate "Chinese into Chinese" and hand back the
  original if you let them (measured). pory detects that from the reported language or from
  the translation being identical to the input, and re-issues the request against the
  secondary target — but only in auto mode.

## Backends

| Backend | API key | Reachable from China | Notes |
|---|---|---|---|
| `msedge` | no | yes | Microsoft's Edge endpoint. First in the default traditional chain |
| `transmart` | no | yes | Tencent's interactive translation. Measured fastest (~0.2 s) |
| `mymemory` | no | yes | 1000 requests/day, 400 characters each. Adding an email raises the quota |
| `google` | no | needs a proxy | Good quality, unofficial endpoint, rate-limits by IP |
| `ai` | yes | depends | Best quality, understands context, preserves formatting. Slower and paid |

An AI provider is one entry under `[ai.<name>]`, and it can carry several models; the chain
becomes `provider:model` pairs, so a failure names the exact model that failed. The protocol
is **OpenAI-compatible** — change `base_url` and `models`:

| Provider | `base_url` |
|---|---|
| DeepSeek | `https://api.deepseek.com/v1` |
| Kimi | `https://api.moonshot.cn/v1` |
| Zhipu | `https://open.bigmodel.cn/api/paas/v4` |
| SiliconFlow | `https://api.siliconflow.cn/v1` |
| Local Ollama | `http://localhost:11434/v1` |

Every backend has a **30-second request timeout**. Without one, a hung connection waits
for the kernel's TCP timeout (~130 s on Linux), which looks exactly like a freeze — and
the fallback chain never gets a chance, because falling back only happens after the
previous backend *reports* a failure.

## Output

```
↳ hello world                                       ← echoed input (only when reading a pipe or file)
你好世界                                             ← translation (stdout, bold in a terminal)
◈ zh-CN ⇄ en · msedge · 10 blocks · cache 9/10 · 1.0s   ← footer (stderr, pinned to the bottom)
```

The footer is the honest part of the interface:

- `⇄` means the direction is decided by detection; `›` means it is fixed
- `bad:m → sf:tencent/Hunyuan-MT-7B` means **a fallback happened**, and the whole line
  turns amber
- `10 blocks` is the real chunk count, computed locally before any request is sent
- `cache 9/10` means "9 chunks served from cache out of 10"
- `1.0s` is the measured wall-clock time for the whole call, chunking and cache writes
  included. A cache hit shows `0.0s` — that is an honest report of "instant", not a
  rounding artifact

While a request is in flight you get a braille thinking spinner with the **measured**
elapsed time — no fabricated progress bar, because we genuinely don't know how long a
network request will take.

Full rationale, plus the font and character-width measurements behind the layout:
[docs/output.md](docs/output.md).

## Design notes

Why the cache is JSON and not SQLite, why the cache key is SHA-256, why tokio runs
single-threaded, why decoration goes to stderr, and how the two-mode routing works:
[docs/design.md](docs/design.md) and [docs/mode-routing-design.md](docs/mode-routing-design.md)
*(written in Chinese for now)*.

## Contributing

Issues and pull requests are welcome. A few things that make a patch easy to accept:

- `cargo test` and `cargo clippy --all-targets` should both be clean
- Comments in the source are in Chinese; `--help` text is English only (it is the user
  interface, and the project deliberately keeps it to one language)
- New features must be **opt-in**: with no configuration, the code path and behavior must
  be identical to before the feature existed

## License

[MIT](LICENSE)
