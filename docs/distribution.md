# 发布与更新方案

> 2026-09-20 调研。状态：**方案讨论中，尚未动手**。
> 前提：仓库已公开（`github.com/harukizmoe/pory`），版本 0.1.0，单提交、无 CI。

## 结论先行

「发布到 AUR / apt / winget」不是一个选择题，而是**一条三层流水线**：

```
构建层   GitHub Actions 矩阵 + 静态链接 + Release 资产     ← 地基，必须先有
分发层   AUR · deb · winget · scoop · MSI · tar.gz/zip
更新层   谁负责升级 —— 由「用户怎么装的」决定，不是由代码决定
```

**地基没打，上面两层都立不住**：AUR 的 `-bin` 包、winget 的 zip、scoop 的清单，全部要求
"已有可下载的预编译资产"。所以顺序不能颠倒。

## 核心区分：更新的归属权

**不建议把自更新做进 `pory` 本体。** 理由不是"麻烦"，而是它会把"没有更新机制"
变成"两套更新机制打架"：

- 用户 `pacman -S pory` 安装 → 文件在 `/usr/bin`，root 所有
- 再跑 `pory update` → 往同一位置写：轻则权限报错，重则覆盖出一个
  pacman 认为"已被用户修改"的二进制，**下次 `pacman -Syu` 会拒绝覆盖**

| 用户的装法 | 谁负责升级 | 成本 |
|---|---|---|
| pacman / apt / winget / scoop | **包管理器自己** | 零代码 |
| `curl \| sh` / `irm \| iex`（安装器脚本） | `pory-update` 独立程序 | 零代码（cargo-dist 的 `install-updater`） |
| `cargo install` | 用户手动重装 | 零代码 |

三条路都不需要 `pory` 本体写一行更新代码 —— 唯一真缺口是「安装器脚本装的人」，
他们没有任何升级通道，`pory-update` 正好补这一格，而且**它是独立的第二个二进制**，
不进 `pory`，所以本体的体积和启动时间一个字不变。

## 各渠道对比

| 渠道 | 用户拿到什么命令 | 一次性成本 | 每次发版成本 | 审核 | 覆盖 |
|---|---|---|---|---|---|
| **cargo-dist（`dist init`）** | `curl \| sh`、`irm \| iex` | 极低（一条命令生成 CI） | 零（打 tag 自动跑） | 无 | Linux + Windows + macOS |
| **crates.io** | `cargo install pory` | 极低 | 一行 `cargo publish` | 无 | Rust 用户 |
| **AUR** | `yay -S pory-bin` | 中（需独立 SSH key、注册 AUR 账号） | 自动（工作流改 pkgver + sha256 后 push） | 无 | Arch 用户 |
| **Scoop** | `scoop install pory` | 低（自建 bucket 仓库，一个 JSON） | 自动 | 无 | Windows 开发者 |
| **winget** | `winget install harukizmoe.pory` | **高**（首发必须手工 + classic PAT + PR） | 自动（`wingetcreate update`） | **人工审核 1–5 个工作日** | 全部 Windows 用户 |
| **apt 仓库** | `apt install pory` | **很高**（GPG 签名 + Pages 托管 + 多 codename） | 中 | 无 | Debian / Ubuntu |
| **MSI（cargo-wix）** | 双击安装 | 中 | 低 | 无 | Windows 非开发者 |

## 推荐路线（按性价比排序）

1. **`dist init` 一把梭** —— 一条命令生成 CI、sh/ps1 安装器、checksums、Release 自动上传，
   顺带可开关 `install-updater`。覆盖"Linux 用户"和"Windows 用户"绝大多数场景，之后零维护。
2. **AUR `pory-bin`** —— 用户是 Arch 用户（你自己就是），成本中、回报直接。
   用预编译 tar.gz，PKGBUILD 简单、用户机器上不用装 Rust 工具链。
   （也可以另发一个从源码构建的 `pory`，Arch 偏好源码包，但每次升级要用户编译。）
3. **winget + Scoop** —— Windows 主渠道。Scoop 便宜先做；winget 影响大但首发最麻烦。
4. **apt 仓库** —— 成本最高、收益最低（个人工具，企业批量部署才是 apt 的主场）。
   可降级为"只管生成 `.deb` 放进 Release"，用户 `dpkg -i` 手动装，但没有 `apt upgrade`。

## 前置与坑

- **目前没有 CI**。这是第一步，不是可选步骤 —— 没有交叉编译矩阵就没有预编译资产。
- **静态链接（musl）需要实测**。pory 用 rustls 不带 OpenSSL，但仍动态链 glibc；
  在 Arch 上编的二进制 glibc 版本很新，**在 Debian stable / 老 Ubuntu 上可能跑不起来**。
  对策是同时出 `x86_64-unknown-linux-gnu` 和 `-musl` 两个构建（cargo-dist 默认就这么干）。
  ⚠️ musl 构建需要在 WSL 里装 `musl` 与 C 工具链（`sudo pacman -S musl`），**动手前会先问**。
  另需注意 musl 目标下的 DNS 解析与 `ring`/`aws-lc-rs` 的 C 编译依赖。
- **AUR 需要一把独立的 SSH key**（Arch Wiki 明确建议不要复用现有 key），
  公钥填到 AUR 账号，私钥配成 `Host aur.archlinux.org`。
- **winget 的首个版本必须手工提交**：`wingetcreate new` 是交互式设计，
  官方明确说无法可靠地在 CI 里自动化；后续版本才能用 `wingetcreate update` 自动 bump。
  另外 winget **只接受 `.zip`**，`.tar.gz` 不行。
- **MSI 不签名会触发 SmartScreen**「未知发布者」警告。个人项目通常直接跳过 MSI，
  用 zip + winget/scoop 更顺。签名还涉及证书费用，暂不考虑。
- **发布后要观察默认后端的配额**：MyMemory 是免 Key 的匿名接口，
  公开分发后如果用户量上来，要确认它的限额是按 IP 还是全局共享 —— 这个需要实测，
  不能靠猜。若出现频繁限流，就要在文档里把 `ai` 后端提为推荐项。

## 对项目原则的影响

**整套发行变更全部落在仓库外围**（`.github/workflows/`、`Cargo.toml` 的 dist 配置、
各平台的清单文件），`src/` 一行不动 —— 二进制体积、启动时间、单次调用行为都不变。
唯一会动到本体的是「自更新做进 `pory`」，而这一条上面已论证不做。

## 待定

- [ ] 是否接受为发布引入 GitHub Actions（新增 CI 目录，不动 `src/`）
- [ ] 渠道优先级：是否按上面的四步走
- [ ] musl 静态链接是否实测（需要 `sudo pacman -S musl`）
- [ ] 是否发 crates.io（包名占位，注意 crates.io 发布不可删除、只能 yank）
