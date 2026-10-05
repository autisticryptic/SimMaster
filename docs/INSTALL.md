# 安装与部署指南

## 1. 支持范围与安全边界

本次修复统一了**离线发布包安装器**与**在线下载 bootstrap**。本地构建和 CI 使用同一个
打包入口；主程序、前端、安装器、主服务单元和设备资源来自同一次打包，不再下载分支上的
unit 来拼接另一个版本的 Release。

- 支持 Linux/systemd，`aarch64` / `x86_64`，固定 `/opt/simadmin` 和 `simadmin.service`。
- 需要 root、Python **3.8+**；在线 bootstrap 另需 `curl`。不自动安装系统依赖或 lpac。
- **默认只安装文件**并执行 `daemon-reload`，不启用、不启动主服务，不调用设备资源安装，
  不操作 ModemManager、NetworkManager 或设备。主服务正在运行时，默认拒绝覆盖。
- `--activate` 是明确授权的**维护窗口操作**：允许停止已有主服务，安装后启用并启动它。
  **SimAdmin 启动本身可能根据既有配置启动 MM 或操作硬件**，不能将此选项视为无扰动升级。
- 不改写配置、不切换 modem backend、不清空数据库、不替换 catalog/lpac/E911 secret。
  不支持自定义安装路径、服务名或配置环境；自定义主 unit/配置 drop-in 请使用人工部署。
  已有 unit 的非注释设置与包内 unit 不同就拒绝，避免把已关闭的设备/网络操作权限改回开启；
  同时检查 `/etc`、`/run`、`/usr/lib` 与非 usrmerge 系统的 `/lib` systemd 目录。
- 旧包缺少 `install.sh`、主 unit 或完整清单时会拒绝安装，而不是回退到 raw 分支文件。
- 已加入临时目录与假命令行为测试；**没有执行实机安装验收，也不代表已有 Release 已重新打包**。
  `uninstall.sh` 和 Web 在线升级/OTA 应用流程不在本次修复范围，仍不要使用。

系统 D-Bus、内核驱动、QMI/MBIM 工具、MM/NM 等按设备和所选 backend 自行准备，见
[运行环境与系统管理](./ENVIRONMENT.md)。native 部署不要为了满足旧教程而安装/启动 MM。

## 2. 构建与统一打包

构建机需要 Rust/Cargo、Node.js/pnpm 和 Python 3。以 ARM64 musl 为例：

```bash
cd frontend
pnpm install --frozen-lockfile
pnpm build
cd ../backend
rustup target add aarch64-unknown-linux-musl
# 另行安装 Zig 和 cargo-zigbuild
SQLITE3_STATIC=1 LIBSQLITE3_SYS_USE_PKG_CONFIG=0 \
  cargo zigbuild --release --target aarch64-unknown-linux-musl
cd ..
sh scripts/pack-ota.sh --target aarch64-unknown-linux-musl
```

AMD64 使用 `--target x86_64-unknown-linux-musl`。已有产物可明确传入：

```bash
sh scripts/pack-ota.sh --target x86_64-unknown-linux-musl \
  --binary /path/to/simadmin --frontend /path/to/dist
```

版本默认读取 `VERSION`，commit 来自当前 Git；二进制的 Cargo 版本必须与包版本一致，
安装预检会执行 `simadmin --version` 核对。务必从同一源码版本构建全部内容，不要用旧二进制
拼出新版本元数据。`scripts/build.sh` 的打包阶段及 CI 都调用这个入口。

输出：

```text
release/simadmin-linux-arm64.tar.gz          # 或 simadmin-linux-amd64.tar.gz
release/simadmin-linux-arm64.tar.gz.sha256   # 此包的独立 SHA256 清单
```

包结构：

```text
meta.json                  版本、commit、目标架构、旧 OTA MD5 字段、installer_format=1
SHA256SUMS                 除自身外所有文件的 SHA256（包括安装器和 unit）
simadmin
www/                       包括隐藏静态文件
install.sh                 与 cwd 无关的离线入口
installer.py               Python 标准库实现
system/simadmin.service     同一源码版本的主服务单元
devices/<device>/system/    设备驱动自有资源
```

保留旧 OTA 元数据字段是格式兼容，不代表旧 Web OTA 应用器获得了这些安装安全保证。

## 3. 离线安装（推荐）

通过可信渠道取得同一构建的包与校验清单。GitHub Release 提供合并的 `SHA256SUMS.txt`；
Actions artifact 和本地打包附带单包 `.tar.gz.sha256`。

**先校验整个包，再解包、执行其中代码**。SHA256 是完整性检查，不是数字签名；如果发布方或
校验值来源被篡改，校验值不能证明可信。没有签名发布体系时，应自行核对来源、commit 和摘要。

```bash
# 本地/Actions 单包清单：
sha256sum -c simadmin-linux-arm64.tar.gz.sha256
# Release 清单（只取需要的、精确文件名；应恰好输出一条）：
grep -E '^[0-9a-f]{64}  simadmin-linux-arm64\.tar\.gz$' SHA256SUMS.txt > selected.sha256
[ "$(wc -l < selected.sha256)" -eq 1 ] && sha256sum -c selected.sha256

mkdir simadmin-package
tar --no-same-owner --no-same-permissions -xzf simadmin-linux-arm64.tar.gz -C simadmin-package
```

请把校验失败视为停止条件，不要继续后续命令。以 root 执行（**不必 cd 到包目录**）：

```bash
sh /path/to/simadmin-package/install.sh --check
sh /path/to/simadmin-package/install.sh
```

`--check` 会复制到私有临时目录，检查包路径类型、完整文件清单、版本/commit/架构、MD5
元数据与二进制实际版本。打包和安装均先做不执行程序的 ELF64 little-endian、`e_machine`、
完整头及 program-header 范围检查，不能仅靠 `meta.json` 把 AMD64 程序标成 ARM64。
随后用暂存二进制的只读 `modem-backend-mode --config ...` 解析现有
主配置及 backend/MM handover gate；不会打开数据库、探测硬件或安装文件。
配置路径与标准服务一致：`/data` 存在时取 `/data/config.yaml`，否则取
`/opt/simadmin/config.yaml`，不会误读临时二进制旁边的空配置。不存在配置时使用程序默认值。
这不是完整服务启动或硬件可用性验证。

默认安装成功后主服务仍未启动/启用。设备资源仅保存在 `/opt/simadmin/devices/`，
不会直接写入 `/etc/systemd/system`。首次安装请先按第 6 节审查是否需要这些资源。

## 4. 在线 bootstrap（必须固定版本）

脚本名沿用 `install_latest.sh`，但**不再支持隐式 latest**，也没有旧仓库默认地址。
先取得并审阅可信源码版本的脚本，再显式指定仓库和已发布版本：

```bash
REPO=your-owner/your-repository VERSION=1.2.3 sh ./install_latest.sh --check
# 确认后，默认只安装文件：
REPO=your-owner/your-repository VERSION=1.2.3 sh ./install_latest.sh
```

`your-owner/your-repository`、`1.2.3` 是占位符，需替换为实际可信仓库与版本。
脚本只请求同一个 `releases/download/v<version>/` 下的架构包和 `SHA256SUMS.txt`；
检查摘要、安全解包并核对固定版本/架构后，调用包内离线安装器。下载失败、旧包、错架构、
缺校验文件、版本不符都会失败关闭。不会从 raw 分支、隐式镜像或 lpac 仓库补齐内容。
旧 `ASSET_URL` / `SERVICE_URL` / `RAW_BASE` / `REPO_BRANCH` 覆盖不再支持。

不要使用 `curl | sh` 盲目执行未知脚本。没有新格式发布包时，应从可信源码构建后离线安装，
不能用此入口强装历史 Release。

## 5. 升级、激活与失败恢复

### 维护窗口前

先运行 `--check`，确定版本/commit/架构及所选 backend 符合预期。确认有独立 SSH/串口恢复
通道，停止服务后再备份配置和用户数据。SQLite 在 WAL 活跃时不能只复制主数据库文件。

```bash
systemctl stop simadmin.service
install -d -m 0700 /path/to/backup
# 文件和 DB 是同一配置的两半；按实际存在的路径一起备份。
cp -a /opt/simadmin/data.db* /path/to/backup/
cp -a /data/config.yaml* /path/to/backup/ 2>/dev/null || true
cp -a /opt/simadmin/config.yaml* /path/to/backup/ 2>/dev/null || true
cp -a /data/config.json* /path/to/backup/ 2>/dev/null || true
cp -a /opt/simadmin/config.json* /path/to/backup/ 2>/dev/null || true
cp -a /data/simadmin/e911 /path/to/backup/ 2>/dev/null || true
```

请使用真实的独立备份路径，并核查备份结果。也可在适合的维护流程中使用内置
`simadmin config backup /path/to/backup/config-snapshot.db` 获取配置的两半。
不要把运行时数据或 catalog 塞进发布包。

### 明确激活

安装器可以在明确授权时负责主服务停止/重启：

```bash
sh /path/to/simadmin-package/install.sh --activate
systemctl status simadmin.service --no-pager
journalctl -u simadmin.service -n 100 --no-pager
```

所有包和配置预检、目标文件暂存完成后才停止已有主服务。激活仅针对 `simadmin.service`，
检查它在启动后 5 秒内保持 active；这**不是 HTTP、IMS、SIM 或硬件健康证明**。
不会默认安装/激活 QCM410 资源，也不会执行重启设备、NM unmanaged 配置或 backend 接管。

### 失败恢复范围

文件替换、`daemon-reload`、enable/start 或短暂启动检查失败时，安装器尝试恢复原来的
二进制、`www`、`meta.json`、设备资源目录和主 unit，并恢复原主服务启用/运行状态。
配置、数据库、catalog、lpac 和 E911 状态不属于替换集合，**不会用旧快照覆盖用户数据**。

这不是跨文件系统原子事务，也不能回滚断电/SIGKILL、运行中服务产生的数据库迁移、已经
发生的硬件状态变化。升级前的配对备份仍然必需；必要时由管理员评估 schema 兼容后恢复。
回滚本身失败会报 `ROLLBACK INCOMPLETE` 并保留 `.install-*` 和 `.simadmin-install-*`
备份路径；停止重试，保留目录和日志人工恢复，不要清空 `/opt/simadmin`。

## 6. 可选设备资源与 lpac

只有明确知道设备需要哪些资源时才进入此步骤，安排维护窗口。包内设备边界仍由驱动负责：

```bash
/opt/simadmin/simadmin device-init --dry-run
/opt/simadmin/simadmin install-device-resources --staging-dir /opt/simadmin
```

**当前后端资源 CLI 有独立限制**：某些驱动失败只输出文字、退出码仍为 0；必须检查输出、
目标文件和 unit 状态，不能仅凭 shell 成功断言安装成功。因此安装器不会把它放进可回滚
主事务。该命令即使不带 `--activate` 也可能启用设备单元；native 路径还可能停用自己的
MM recovery 单元，并非只读操作。

QCM410 资源的立即激活选项 `--activate` 可能停止/重启 MM，**仅限另行明确授权的维护操作**。
默认不执行。不要在未知硬件上复制 QCM410 unit，也不要预置猜测端口名的 udev 规则。
如需重启设备让 boot ordering 生效，也应由管理员另行决定，而非安装器自动执行。

eSIM 管理可能需要架构/libc 兼容且支持所需 APDU backend 的 lpac。自行审核来源及能力后安装
到 `/opt/simadmin/lpac/lpac`，配套库放入 `/opt/simadmin/lpac/lib/`。普通 SIM 不要求 lpac；
安装器不下载、不替换已有 lpac。

## 7. 首次访问与诊断

主服务实际启动并检查日志后，访问 `http://设备IP:3000`。没有默认管理员密码，首次访问设置
密码。可选 schema v7 sealed `carrier-bundles.sqlite3` 不在安装器替换集合内；不存在时可在
WebUI 的运营商 IMS Profile 页面按现有流程选择并安装 catalog。

“安装不了”时请保留：执行的完整命令（去掉凭据）、退出码、stdout/stderr、`uname -m`、
Python 版本、包的 SHA256 和 `meta.json`，以及是否为标准配置路径/有无 systemd drop-in。
不要发送密码、SIM 密钥、完整数据库或 E911 secret。安装器不会以忽略错误的方式继续覆盖。

仓库内可重复的无硬件回归：

```bash
python3 -m unittest discover -s scripts/tests -p 'test_installer.py' -v
```

测试使用临时 roots、只做格式验证的 ELF fixture 和假 systemctl/curl/程序输出；不运行真实
主机安装或生产二进制，不连接设备。当前 35 项测试通过，包含关闭的安全环境变量、
`/lib` vendor drop-in、错架构/截断 ELF 和失败回滚的回归。
