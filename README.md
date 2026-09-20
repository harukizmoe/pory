# Pory

**Translate text right in your terminal — no API key, no browser, no copy-paste.**

`pory` is a small command-line translator for people who live in a shell. It works out of
the box, remembers what it has already translated, and tells you honestly which backend
actually produced the result.

> Named after [Porygon](https://en.wikipedia.org/wiki/Porygon) — a creature made of data
> that adapts to any environment. Translation is the same job: turning one symbol system
> into another.

[English](README.md) · [简体中文](README.zh-CN.md) · [日本語](README.ja.md)

```console
$ pory "今天天气不错"
It's a nice day today
◈ zh-CN ⇄ en · mymemory · 1 block · cache 0/1
```

That `◈` line is the only decoration. It says which languages, which backend, how many
chunks, and how much came from cache — and it goes to **stderr**, so redirecting stdout
gives you nothing but the translation.

## Features

- **Zero configuration** — the default backend (MyMemory) needs no API key and is
  reachable from mainland China. Install it and it works.
- **Pluggable backends** — MyMemory, Google, or any OpenAI-compatible API
  (DeepSeek, Kimi, Zhipu, local Ollama…). One line of config switches between them.
- **Automatic fallback** — if the primary backend is missing a key, out of quota or times
  out, the next one takes over. A warning tells you it happened and the footer shows the
  real path — silent fallback would leave you thinking you were using an LLM when you
  were not.
- **Bidirectional by default** — set a primary and a secondary language. In auto mode,
  text already in your primary language is translated *into* your secondary one, instead
  of being handed back to you unchanged.
- **Pipe-friendly** — `cat notes.md | pory -t en`. Colors and the progress animation only
  appear when the stream is a real terminal, so pipes, CI and redirections stay byte-clean.
- **Smart chunking** — long text is split on paragraph → sentence → hard cut, so nothing
  gets chopped mid-word.
- **Chunk-level cache** — a sentence you translated last week is free today. Cache hits
  return in about 4 ms.
- **Small and fast** — a single static binary (~3.6 MB), ~3 ms startup, no runtime
  dependencies.
- **Pure Rust + rustls** — no OpenSSL, so cross-compiling to musl or ARM doesn't drag in
  system libraries.

## Install

```bash
cargo install --git https://github.com/harukizmoe/pory
```

Or build from source:

```bash
git clone https://github.com/harukizmoe/pory
cd pory
cargo build --release        # binary at target/release/pory
```

## Quick start

```bash
pory "今天天气不错"            # -> It's a nice day today
pory "hello world"             # -> 你好世界          (auto: English in, Chinese out)
pory "Good morning" -t ja      # -> おはようございます
pory "你好，世界" -t en         # -> Hello, world.
pory "こんにちは" -f ja -t zh   # explicit source and target
cat notes.md | pory -t en      # read from stdin
pory "hello" --plain           # translation only, no footer, no animation
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
| `-b, --backend <NAME>` | `mymemory` / `google` / `ai`. See below — it has two meanings |
| `--plain` | Print the translation only: no color, no footnote, no animation |
| `--no-cache` | Don't use the cache: neither read existing entries nor write new ones |
| `--refresh` | Ignore the existing cache, re-translate and update it |
| `--clear-cache` | Clear the translation cache and exit |
| `--init` | Write a sample config file and exit |

### `-b` has two meanings

Which one you get depends on whether there is any input:

```bash
pory -b google "text"           # use google for this call
echo "text" | pory -b google    # same — piped input also counts as "this call"
pory -b google                  # no text → save it to the config as the new default
```

The last form prints `✓ 已把默认后端设为 google` and exits, rewriting only the `backend`
line in your config file — comments and everything else stay untouched. Pass
`pory -b mymemory` to switch back.

If you combine `-b` with another flag but no text (`pory -b google -t ja`), pory prints a
usage hint instead of silently editing your config — that case is almost always a typo.

### Supported languages

`zh` `zh-tw` `en` `ja` `ko` `fr` `de` `es` `ru` `it` `pt` `ar` `th` `vi`

## Configuration

Everything is optional. Run `pory --init` to write
`~/.config/pory/config.toml` (on Windows: `%APPDATA%\pory\`):

```toml
backend = "mymemory"
fallback = ["mymemory"]     # tried in order when the primary backend fails
primary = "zh"              # your language: the target when -t is not given
secondary = "en"            # the other language; leave "" to disable swapping
source = "auto"
cache = true                # false is equivalent to always passing --no-cache

# MyMemory: adding an email raises the free quota
# mymemory_email = "you@example.com"

# Google: you can point this at your own proxy
# google_endpoint = "https://your-worker.workers.dev/translate_a/single"

# AI backend (OpenAI-compatible)
[ai]
base_url = "https://api.deepseek.com/v1"
api_key = ""
model = "deepseek-chat"
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

It also costs nothing extra: only the same-language case needs a second request.

## Backends

| Backend | API key | Reachable from China | Notes |
|---|---|---|---|
| `mymemory` | no | yes | Default. 1000 requests/day, 400 characters each |
| `google` | no | needs a proxy | Good quality, unofficial endpoint, rate-limits by IP |
| `ai` | yes | depends | Best quality, understands context, preserves formatting. Slower and paid |

The AI backend is written once and works with many providers because it speaks the
**OpenAI-compatible protocol** — change `base_url` and `model`:

| Provider | `base_url` |
|---|---|
| DeepSeek | `https://api.deepseek.com/v1` |
| Kimi | `https://api.moonshot.cn/v1` |
| Zhipu | `https://open.bigmodel.cn/api/paas/v4` |
| Local Ollama | `http://localhost:11434/v1` |

Every backend has a **30-second request timeout**. Without one, a hung connection waits
for the kernel's TCP timeout (~130 s on Linux), which looks exactly like a freeze — and
the fallback chain never gets a chance, because falling back only happens after the
previous backend *reports* a failure.

## Output

```
↳ hello world                                  ← echoed input (only when reading a pipe or file)
你好世界                                        ← translation (stdout, bold in a terminal)
◈ zh-CN ⇄ en · mymemory · 1 block · cache 0/1   ← footer (stderr, pinned to the bottom)
```

The footer is the honest part of the interface:

- `⇄` means the direction is decided by detection; `›` means it is fixed
- `ai → mymemory` means **a fallback happened**, and the whole line turns amber
- `3 blocks` is the real chunk count, computed locally before any request is sent
- `cache 3/3` means "3 chunks served from cache out of 3"

While a request is in flight you get a braille thinking spinner with the **measured**
elapsed time — no fabricated progress bar, because we genuinely don't know how long a
network request will take.

Full rationale, plus the font and character-width measurements behind the layout:
[docs/output.md](docs/output.md).

## Design notes

Why the cache is JSON and not SQLite, why the cache key is SHA-256, why tokio runs
single-threaded, why decoration goes to stderr, and how to add a backend in one file:
[docs/design.md](docs/design.md) *(written in Chinese for now)*.

## Contributing

Issues and pull requests are welcome. A few things that make a patch easy to accept:

- `cargo test` and `cargo clippy --all-targets` should both be clean
- Comments in the source are in Chinese; `--help` text is English only (it is the user
  interface, and the project deliberately keeps it to one language)
- New features must be **opt-in**: with no configuration, the code path and behavior must
  be identical to before the feature existed

## License

[MIT](LICENSE)
