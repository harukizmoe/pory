# Pory

**ターミナルでそのまま翻訳 —— キー不要ですぐ使え、AI を繋げばさらに賢く。
ブラウザもコピペも要りません。**

`pory` は、シェルから離れずに翻訳や AI 辞書検索を使えるコマンドラインツールです。
翻訳はすぐ使え、翻訳済みの内容を覚え、
**今回の結果をどのバックエンドが生成したのか**を正直に表示します。

[English](README.md) · [简体中文](README.zh-CN.md) · [日本語](README.ja.md) · [Issue](https://github.com/harukizmoe/pory/issues)

```console
$ pory "今天天气不错"
The weather is nice today
◈ zh-CN ⇄ en · msedge · 1 block · cache 0/1 · 0.5s
```

`◈` のステータス行は **stderr** に出るため、stdout のリダイレクト先には翻訳文だけが残ります。

## インストール

Rust と Cargo が必要です。

```bash
cargo install --git https://github.com/harukizmoe/pory --locked
```

ソースからビルドする場合：

```bash
git clone https://github.com/harukizmoe/pory
cd pory
cargo build --release
```

## クイックスタート

```bash
pory "今天天气不错"            # -> The weather is nice today
pory "hello world"             # -> 你好，世界（英語から中国語へ）
pory "Good morning" -t ja      # -> おはようございます
pory "你好，世界" -t en         # -> Hello, world.
cat notes.md | pory -t en      # 標準入力から翻訳
pory -b traditional "hello"    # 通常の機械翻訳を使う
pory "hello" --plain           # 翻訳文だけを出力
pory dict hello                  # AI 生成の辞書項目
pory dict "break the ice"        # 句や慣用表現を検索
```

既定の `primary = "zh"`、`secondary = "en"` では、オプションなしで双方向に翻訳できます。
別の言語ペアを使う場合は設定を変更してください。

## 特徴

- **API キー不要** —— 設定ファイルなしで使えます。AI プロバイダは任意で、失敗したバックエンドは次へ切り替わり、実際の経路を表示します。
- **プロバイダを選べる** —— キー不要の翻訳サービスと OpenAI 互換 API に対応し、複数モデルも設定できます。
- **双方向翻訳** —— `primary` と `secondary` を設定すると、auto モードで原文の言語に応じて訳す方向を選びます。
- **パイプ対応** —— 翻訳文は stdout、状態は stderr に出力します。`--plain` で翻訳文だけにできます。
- **AI 辞書検索** —— 中国語・日本語・英語の語句を検索し、利用可能な発音、品詞、語義、例文を表示します。結果は AI 生成で、権威ある辞書ではありません。
- **分割とキャッシュ** —— 長文は段落や文で分割し、完了したブロックを再利用できます。
- **シェル統合** —— fish / bash / zsh のエイリアスと補完に対応。`--uninstall` で追加した設定を削除できます。
- **Rust + rustls** —— OpenSSL は不要です。

## 使い方

```
Usage: pory [OPTIONS] [TEXT] [COMMAND]
```

| オプション/コマンド | 説明 |
|---|---|
| `[TEXT]` | 翻訳するテキスト。省略すると stdin から読みます（パイプ対応） |
| `dict <TERM>` | 単語や語句を検索します。複数語は引用符で囲みます |
| `-t, --target <LANG>` | 訳先の言語（`zh` / `en` / `ja` など） |
| `-f, --from <LANG>` | 原文の言語。既定は自動判定 |
| `-b, --backend <MODE>` | `ai` / `traditional`。**意味が 2 通りあります（下記）** |
| `--plain` | 翻訳文のみ：色なし、フッターなし、アニメーションなし |
| `--no-cache` | 翻訳と辞書項目のキャッシュを読み書きしない |
| `--refresh` | 既存キャッシュを無視して結果を再生成する |
| `--timeout <SECONDS>` | 操作全体の時間上限。既定は 300 秒（1〜86400 秒） |
| `--clear-cache` | 翻訳と辞書項目のキャッシュを消して終了 |
| `--init` | サンプルの設定ファイルを書き出して終了（既存のものは上書きしません） |
| `--init-shell <SHELL>` | シェル統合を導入：`fish` / `bash` / `zsh` |
| `--print` | `--init-shell` と併用：導入せずスクリプトだけを出力 |
| `--uninstall` | pory がこのシステムに加えたものをすべて消して終了 |

### 辞書検索

```bash
pory dict hello
pory dict "break the ice"
```

訳語の方向は `primary` / `secondary` の自動ルールに従い、`-f` / `-t` で個別指定できます。検索に成功すると、見出し語と読みの後に主要な品詞と簡潔な直接訳を並べて表示し、その後に語義と例文を表示します。AI 検索に失敗すると通常の翻訳へフォールバックし、「翻訳のみ・辞書項目ではない」と表示します。このサブコマンドでは `-b` と `--plain` は使えません。キャッシュは `--no-cache` / `--refresh` で制御できます。

### `-b` の使い方

```bash
pory -b traditional "text"  # 今回だけ使う
pory -b traditional          # 既定のモードに設定
```

テキストがなければ設定の `mode` だけを更新します。パイプ入力は今回のテキストとして扱われます。

### 対応言語

`zh` `zh-cn` `zh-tw` `en` `ja` `ko` `fr` `de` `es` `ru` `it` `pt` `ar` `th` `vi`

### シェル統合

```bash
pory --init-shell fish     # -> ~/.config/fish/completions/pory.fish
pory --init-shell bash     # -> 補完ファイル + ~/.bashrc に `alias pr=pory` を追記
pory --init-shell zsh      # -> スクリプトを出力 + ~/.zshrc に同じエイリアスを追記
```

短縮名 `pr` と補完を追加します。変更は元に戻せ、pory が作成していないファイルは上書きしません。
`pr` は POSIX コマンド名でもあるため、そちらを使う場合は `command pr` を実行してください。
zsh は補完ファイルの場所が `$fpath` に依存するため、スクリプトを出力します（fish / bash は実機確認済み、zsh は未確認）。

### アンインストール

```bash
pory --uninstall
```

追加したシェル設定とキャッシュを削除します。API キーを含む可能性がある設定ファイルは確認後に削除し、非対話環境では残します。バイナリは Cargo またはパッケージマネージャーから削除してください。

## 設定

**すべて任意です。** `pory --init` を実行すると
`~/.config/pory/config.toml`（Windows では `%APPDATA%\pory\`）を生成します：

```toml
mode = "ai"                 # ai | traditional
primary = "zh"              # 第一言語：-t を指定しないときの訳先
secondary = "en"            # 第二言語："" にすると相互翻訳を無効化
source = "auto"
cache = true                # false は翻訳と辞書項目のキャッシュを無効にする

# ── AI プロバイダ（OpenAI 互換、複数設定できます）──
[ai]
order = ["zhipu", "siliconflow"]   # この順に試します。api_key のないものは自動でスキップ

[ai.siliconflow]
base_url = "https://api.siliconflow.cn/v1"
api_key = ""
models = ["tencent/Hunyuan-MT-7B", "Qwen/Qwen3-8B"]
thinking = false            # 既定は無効。true にするとこのプロバイダで有効

# ── 伝統的な機械翻訳（キー不要、常に利用可能）──
# order は mode = "traditional" のチェーン順と、mode = "ai" で AI の後ろに
# 繋がる保険の順序の両方を決めます。
[traditional]
order = ["msedge", "transmart", "mymemory", "google"]
```

設定ファイルがなくても組み込みの既定値で動きます —— **それはエラーではありません**。
キャッシュも同じで、読めない・壊れている場合は黙って空のキャッシュに落ちます。
キャッシュのせいで道具が使えなくなる、という事態は起こしません。
Unix では `pory --init` が所有者だけに読み書きを許す権限で設定ファイルを作成します。
手動作成した設定や古い設定では、`chmod 600 ~/.config/pory/config.toml` で API キーを保護してください。

### 言語の方向

auto モードでは、検出した原文の言語に応じて `primary` と `secondary` の間を翻訳します：

```bash
pory "hello world"     # 原文は英語 → 中国語へ
pory "你好，世界"       # 原文がすでに中国語 → 英語へ
```

`-f` または `-t` を明示すると方向は固定されます。`secondary` を空にすると双方向切り替えを無効にできます。

## バックエンド

キー不要の内蔵バックエンドは `msedge`、`transmart`、`mymemory`、`google` です。AI は OpenAI 互換 API（DeepSeek、Kimi、Zhipu、SiliconFlow、Ollama など）に対応し、`[ai.<name>]` に `base_url` とモデル名を設定します。プロバイダごとに複数のモデルを設定でき、記載順に試してから次のプロバイダへ進みます。

各バックエンドのリクエストは **30 秒**でタイムアウトします。翻訳全体の上限は既定で
**300 秒**で、すべての分割テキストとフォールバックで共有されます。上限に達すると
エラーを表示し、完了済みの分割分はキャッシュに保存します。長文では
`--timeout 900`（15 分、最大 86400 秒）に延長できます。個別リクエストのタイムアウト時は次へ切り替えます。

## コントリビュート

issue も PR も歓迎します。パッチを受け入れやすくするための習慣がいくつかあります：

- `cargo test` と `cargo clippy --all-targets` の両方がクリーンであること
- ソースのコメントは中国語、`--help` のテキストは**英語のみ**
  （ユーザーインターフェースなので、意図的に 1 言語に絞っています）
- 新機能は必ず **opt-in**：設定なしのとき、コード経路と挙動が
  機能追加前と完全に同じであること

## ライセンス

[MIT](LICENSE)
