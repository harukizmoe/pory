# Pory

**在终端里直接翻译 —— 开箱免 Key，也能接上你自己选的 AI。不用开浏览器，不用复制粘贴。**

`pory` 是一个为命令行用户提供翻译与 AI 词典查词的工具。翻译功能开箱即用，会记住已翻译内容，
并且**如实告诉你这次结果由哪个后端生成**。

[English](README.md) · [简体中文](README.zh-CN.md) · [日本語](README.ja.md) · [问题反馈](https://github.com/harukizmoe/pory/issues)

```console
$ pory "今天天气不错"
The weather is nice today
◈ zh-CN ⇄ en · msedge · 1 block · cache 0/1 · 0.5s
```

`◈` 状态行显示语言、后端和耗时，并写入 **stderr**；重定向 stdout 只会得到译文。

## 安装

需要 Rust 和 Cargo：

```bash
cargo install --git https://github.com/harukizmoe/pory --locked
```

或从源码构建：

```bash
git clone https://github.com/harukizmoe/pory
cd pory
cargo build --release
```

## 快速上手

```bash
pory "今天天气不错"            # -> The weather is nice today
pory "hello world"             # -> 你好，世界（英文输入，译成中文）
pory "Good morning" -t ja      # -> おはようございます
pory "你好，世界" -t en         # -> Hello, world.
cat notes.md | pory -t en      # 从标准输入读取
pory -b traditional "hello"    # 使用传统机翻
pory "hello" --plain           # 只输出译文
pory dict hello                  # AI 生成的词典词条
pory dict "break the ice"        # 查询词组或习语
```

默认 `primary = "zh"`、`secondary = "en"`，不带参数也能双向翻译。可在配置中更换语种。

## 特性

- **免 Key 开箱可用** —— 无需配置文件即可运行；AI 提供商可选，后端失败会自动回退并显示实际路径。
- **可接入不同服务** —— 支持免 Key 机翻和 OpenAI 兼容 API；每个 AI 提供商可配置多个模型。
- **双向翻译** —— 设置 `primary` 与 `secondary` 后，auto 模式会按检测出的原文语种选择方向。
- **适合管道** —— 译文写入 stdout，状态写入 stderr；`--plain` 只输出译文。
- **AI 词典查词** —— 支持查中文、日语、英语词条；按可用信息展示读音、词性、义项和例句。结果由 AI 生成，不保证权威性。
- **切块与缓存** —— 长文本尽量按段落和句子切分，已完成的文本块可复用缓存。
- **Shell 集成** —— 支持 fish、bash、zsh 的别名与补全；`--uninstall` 可撤销 pory 添加的内容。
- **Rust + rustls** —— 不依赖 OpenSSL。

## 用法

```
Usage: pory [OPTIONS] [TEXT] [COMMAND]
```

| 参数/命令 | 说明 |
|---|---|
| `[TEXT]` | 要翻译的文本。省略则从 stdin 读取（支持管道） |
| `dict <TERM>` | 查询单词或词组；多词词条需加引号 |
| `-t, --target <LANG>` | 目标语言，如 `zh` / `en` / `ja` |
| `-f, --from <LANG>` | 源语言，默认 `auto` 自动检测 |
| `-b, --backend <MODE>` | `ai` / `traditional`。**它有两种含义，见下** |
| `--plain` | 只输出纯译文：不加颜色、不要脚注、不要动画 |
| `--no-cache` | 本次不读写翻译或词条缓存 |
| `--refresh` | 忽略已有缓存，强制重新生成结果 |
| `--timeout <SECONDS>` | 整次操作的总时限，默认 300 秒（范围 1–86400 秒） |
| `--clear-cache` | 清空翻译与词条缓存并退出 |
| `--init` | 生成示例配置文件并退出（已存在时绝不覆盖） |
| `--init-shell <SHELL>` | 安装 shell 集成：`fish` / `bash` / `zsh` |
| `--print` | 配合 `--init-shell`：只打印脚本，不安装 |
| `--uninstall` | 清掉 pory 在这台机器上加的所有东西，然后退出 |

### 词典查词

```bash
pory dict hello
pory dict "break the ice"
```

释义语向遵循 `primary` / `secondary` 自动规则，也可用 `-f` 和 `-t` 单独指定。成功时，词头行会在词头和读音后将主要词性与简短直译并列显示，之后再展示义项与例句。AI 查词失败时会降级为普通翻译，并明确标为“仅翻译，非词条”。此命令不接受 `-b` 或 `--plain`；可用 `--no-cache` 或 `--refresh` 控制缓存。

### `-b` 的两种用法

```bash
pory -b traditional "text"  # 只对本次翻译生效
pory -b traditional          # 设为默认模式
```

没有文本时只更新配置中的 `mode`；管道输入也算本次文本。

### 支持的语种

`zh` `zh-cn` `zh-tw` `en` `ja` `ko` `fr` `de` `es` `ru` `it` `pt` `ar` `th` `vi`

### shell 集成

```bash
pory --init-shell fish     # -> ~/.config/fish/completions/pory.fish
pory --init-shell bash     # -> 补全文件 + 往 ~/.bashrc 追加 `alias pr=pory`
pory --init-shell zsh      # -> 打印脚本 + 往 ~/.zshrc 追加同样的别名
```

安装短名 `pr` 和参数补全；改动可撤销，不会覆盖其他程序创建的文件。`pr` 也有 POSIX 命令同名，运行原命令可用 `command pr`。zsh 会打印补全脚本供你按 `$fpath` 安装；fish 和 bash 已实测，zsh 尚未实测。

### 卸载

```bash
pory --uninstall
```

删除 pory 添加的 shell 配置和缓存；删除可能含 API Key 的配置文件前会询问，非交互环境下会保留。二进制需由 Cargo 或包管理器单独卸载。

## 配置

**全部是可选的**。运行 `pory --init` 生成
`~/.config/pory/config.toml`（Windows 在 `%APPDATA%\pory\`）：

```toml
mode = "ai"                 # ai | traditional
primary = "zh"              # 第一语言（母语）：不指定 -t 时翻成它
secondary = "en"            # 第二语言：与第一语言互翻，留空 "" 即关闭
source = "auto"
cache = true                # false 等价于每次都带 --no-cache（翻译和词条都不读写）

# ── AI 提供商（OpenAI 兼容，可配多个）──
[ai]
order = ["zhipu", "siliconflow"]   # 按此顺序尝试；没填 api_key 的自动跳过

[ai.siliconflow]
base_url = "https://api.siliconflow.cn/v1"
api_key = ""
models = ["tencent/Hunyuan-MT-7B", "Qwen/Qwen3-8B"]
thinking = false            # 默认关闭；改为 true 可按提供商开启思考

# ── 传统机翻（免 Key，永远可用）──
# order 同时决定 mode = "traditional" 时的链顺序，和 mode = "ai" 时接在 AI 后面的保底顺序。
[traditional]
order = ["msedge", "transmart", "mymemory", "google"]
```

找不到配置文件就用内置默认值，**不会报错**。缓存同理：读不到或文件损坏时，
pory 会静默降级为空缓存，绝不会因为缓存出问题就让工具不能用。
Unix 上 `pory --init` 会以仅当前用户可读写的权限创建配置文件；若使用手动创建或旧版配置，
可运行 `chmod 600 ~/.config/pory/config.toml` 保护其中的 API Key。

### 第一语言 / 第二语言互翻

**auto 模式下**（不写 `-f` 或 `-t`），pory 根据原文语种在 `primary` 和 `secondary` 之间选择方向：

```bash
pory "hello world"     # 原文是英文 → 译成中文
pory "你好，世界"       # 原文已是中文 → 改译成英文
```

显式指定 `-f` 或 `-t` 时不切换方向；将 `secondary` 设为空字符串可关闭互翻。

## 后端

内置免 Key 后端包括 `msedge`、`transmart`、`mymemory` 和 `google`。AI 服务支持 OpenAI 兼容接口，例如 DeepSeek、Kimi、智谱、硅基流动和 Ollama；配置写在 `[ai.<name>]`，填写 `base_url` 与模型名即可。每个提供商可配置多个模型，按配置顺序尝试后再切换到下一个提供商。

每个后端请求最多等待 **30 秒**；整次翻译默认最多 **300 秒**，所有文本块和回退请求
共享这份总时限。到时会明确报错，并保存已完成文本块的缓存。长文本可用
`--timeout 900` 增加到 15 分钟（最多 86400 秒）。单个请求超时仍会尝试回退后端。

## 参与开发

欢迎提 issue 和 PR。几条能让补丁更容易被接受的习惯：

- `cargo test` 和 `cargo clippy --all-targets` 都要干净
- 源码注释用中文；`--help` 文本**只用英文**（它是用户界面，本项目刻意只保留一种语言）
- 新功能必须 **opt-in**：不配置时，代码路径与行为要和加这个功能之前完全一致

## 许可证

[MIT](LICENSE)
