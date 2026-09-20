# packaging

各分发渠道的打包文件。目前只有 AUR。

## AUR：`pory-bin`

`aur/pory-bin/PKGBUILD` 是**唯一真相**；AUR 上的 `pory-bin` 仓库只是它的一个副本。

### 为什么是 `-bin` 而不是源码包

预编译包的价值是「1.8 MB 秒装」，而不是让用户为了编一个 3.6 MB 的小工具去拉
几百 MB 的 Rust 工具链。同一份 Release 资产还能让 `cargo binstall pory` 免编译安装。
代价是：**每个版本都要先发一个带资产的 GitHub Release**。

### 发一个新版本

```bash
# 1) 在 WSL 里构建并打包（必须在 WSL 里做，见仓库根的开发说明）
cd /mnt/d/PersonalSpace/worspace/pory
CARGO_TARGET_DIR=~/.cache/pory-target cargo build --release --locked
rm -rf /tmp/pory-rel && mkdir -p /tmp/pory-rel/pory-x86_64-unknown-linux-gnu
cp ~/.cache/pory-target/release/pory LICENSE README.md /tmp/pory-rel/pory-x86_64-unknown-linux-gnu/
cd /tmp/pory-rel
chmod 755 pory-x86_64-unknown-linux-gnu pory-x86_64-unknown-linux-gnu/pory
chmod 644 pory-x86_64-unknown-linux-gnu/README.md pory-x86_64-unknown-linux-gnu/LICENSE
tar -czf pory-x86_64-unknown-linux-gnu.tar.gz pory-x86_64-unknown-linux-gnu
sha256sum pory-x86_64-unknown-linux-gnu.tar.gz | cut -d" " -f1 \
  > pory-x86_64-unknown-linux-gnu.tar.gz.sha256

# 2) 打 tag 并发 Release（资产名必须与 PKGBUILD 里引用的完全一致）
cd /mnt/d/PersonalSpace/worspace/pory
git tag -a v0.1.0 -m "pory 0.1.0" && git push origin v0.1.0
gh release create v0.1.0 \
  /tmp/pory-rel/pory-x86_64-unknown-linux-gnu.tar.gz \
  /tmp/pory-rel/pory-x86_64-unknown-linux-gnu.tar.gz.sha256 \
  --title "pory 0.1.0" --notes "…"

# 3) 把新的 sha256 填回 PKGBUILD（pkgver 同时更新）
```

### 推到 AUR

```bash
# 首次：AUR 上空的 pkgbase 可以直接 clone
git -c init.defaultBranch=master clone ssh://aur@aur.archlinux.org/pory-bin.git ~/aur/pory-bin
cp packaging/aur/pory-bin/PKGBUILD ~/aur/pory-bin/
cp LICENSE ~/aur/pory-bin/
cd ~/aur/pory-bin
makepkg --printsrcinfo > .SRCINFO
git add PKGBUILD .SRCINFO LICENSE
git commit -m "pory-bin 0.1.0-1"
git push

# 之后每次更新：改 pkgver + sha256sums → 重新生成 .SRCINFO → commit → push
```

### 注意事项

- **AUR 只接受推送到 `master` 分支**；**每次 PKGBUILD 元数据变化都要重新生成
  `.SRCINFO`**，否则 AUR 网页上不会显示新版本号（用 `git commit --amend` 补也行）。
- **本地验证**：`makepkg -f` 会真的去 GitHub 下载资产并校验 sha256，跑通即说明
  「Release 资产 + PKGBUILD」这条链是对的。
- `namcap` 是官方 lint 工具，目前 WSL 里没装（需要 sudo 密码）；装上后建议跑一次。
- AUR 需要一把**独立的 SSH key**（`~/.ssh/aur`），公钥贴在 AUR 账号设置里。
