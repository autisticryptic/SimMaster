# 手动安装与部署指南

> 当前仅支持手动安装。一键安装、自动卸载、Release 下载、在线升级和 OTA 应用流程正在
> 重构，仓库中的 `install_latest.sh`、`uninstall.sh`、`scripts/pack-ota.sh` 及相关代码暂不
> 作为可用发布渠道。不要运行来自原仓库或其他未知地址的安装脚本。

<!-- TODO: 仓库地址、发布制品和 OTA 契约稳定后，重新编写并审核自动安装/升级文档。 -->

## 1. 准备环境

目标设备需要 Debian/Linux、systemd、root 权限、system D-Bus、ModemManager、
NetworkManager、QMI 工具和与目标能力匹配的内核驱动。详细要求见
[运行环境与系统管理](./ENVIRONMENT.md)。

构建机需要：

- Rust stable 与 Cargo。
- Node.js、pnpm。
- 交叉构建 aarch64 musl 时需要 Zig 与 cargo-zigbuild，或等价的 musl 工具链。
- 可选：一个由 `carrier_Bundles` 生成、已封存且契约兼容的 schema v7
  `carrier-bundles.sqlite3`。不预置该文件时，SimAdmin 仍可启动，管理员可在 WebUI 中选择
  数据库并在线安装。

## 2. 手动构建

### 前端

```bash
cd frontend
pnpm install --frozen-lockfile
pnpm build
cd ..
```

产物位于 `frontend/dist/`。

### 后端：在目标架构原生构建

```bash
cd backend
cargo build --release
cd ..
```

产物位于 `backend/target/release/simadmin`。

### 后端：交叉构建 aarch64 musl

```bash
rustup target add aarch64-unknown-linux-musl
cargo install cargo-zigbuild
cd backend
SQLITE3_STATIC=1 LIBSQLITE3_SYS_USE_PKG_CONFIG=0 \
  cargo zigbuild --release --target aarch64-unknown-linux-musl
cd ..
```

产物位于 `backend/target/aarch64-unknown-linux-musl/release/simadmin`。请使用适合实际目标
设备架构的二进制，不要把 x86_64 构建产物安装到 aarch64 设备。

## 3. 将产物传到目标设备

以下示例使用文档专用地址 `192.0.2.10`，执行前替换成真实设备地址：

```bash
scp backend/target/aarch64-unknown-linux-musl/release/simadmin \
  root@192.0.2.10:/tmp/simadmin.new
scp -r frontend/dist root@192.0.2.10:/tmp/simadmin-www
scp scripts/simadmin.service root@192.0.2.10:/tmp/simadmin.service
```

也可以使用 ADB、U 盘或其他可信方式传输，但最终至少需要以下三项：

```text
simadmin                 后端可执行文件
frontend/dist/           前端静态资源
simadmin.service         systemd 主服务单元
```

`carrier-bundles.sqlite3` 是可选预置文件；不传输时，首次启动后从 WebUI 的“运营商 IMS
Profile -> 数据库下载”选择 Pixel、iPhone 或 iOS IPCC catalog。

## 4. 安装主服务

SSH 登录目标设备，以 `root` 执行：

```bash
install -d -m 0755 /opt/simadmin /opt/simadmin/www
install -m 0755 /tmp/simadmin.new /opt/simadmin/simadmin
cp -a /tmp/simadmin-www/. /opt/simadmin/www/
install -m 0644 /tmp/simadmin.service \
  /etc/systemd/system/simadmin.service
systemctl daemon-reload
systemctl enable --now simadmin.service
```

检查服务与启动日志：

```bash
systemctl status simadmin --no-pager
journalctl -u simadmin -n 100 --no-pager
```

后端默认从可执行文件同目录读取 `carrier-bundles.sqlite3`。文件不存在时服务以无运营商
catalog 模式启动，Profile 页面会提供数据库选择和下载入口。下载结果经过 schema v7 与封存
状态校验后原子安装到默认路径，并在当前进程中立即生效。如果使用其他位置，需要通过
`serve --carrier-catalog <path>` 指定路径。

## 5. 可选：安装副 QMI/VoLTE 服务

只有目标硬件、内核和 RPMSG 布局符合要求时才执行本节。先在设备上验证：

```bash
/opt/simadmin/simadmin secondary-qmi-init --dry-run
```

在构建机上传文件：

```bash
scp deploy/system/99-simadmin-secondary-qmi.rules \
  root@192.0.2.10:/tmp/99-simadmin-secondary-qmi.rules
scp deploy/system/simadmin-secondary-qmi.service \
  root@192.0.2.10:/tmp/simadmin-secondary-qmi.service
```

在目标设备安装：

```bash
install -d -m 0755 /etc/udev/rules.d
install -m 0644 /tmp/99-simadmin-secondary-qmi.rules \
  /etc/udev/rules.d/99-simadmin-secondary-qmi.rules
install -m 0644 /tmp/simadmin-secondary-qmi.service \
  /etc/systemd/system/simadmin-secondary-qmi.service
udevadm control --reload-rules
systemctl daemon-reload
systemctl enable simadmin-secondary-qmi.service
```

该服务必须在 ModemManager 之前准备 DATA6 端点。启用后建议在维护窗口重启设备，再检查：

```bash
systemctl status simadmin-secondary-qmi --no-pager
journalctl -u simadmin-secondary-qmi -n 100 --no-pager
ls -l /dev/wwan*
```

## 6. 可选：手动安装 lpac

eSIM/eUICC 管理需要与设备架构和 libc 兼容的 `lpac`。请从可信来源自行取得，审核后复制：

```bash
install -d -m 0755 /opt/simadmin/lpac
install -m 0755 /path/to/lpac /opt/simadmin/lpac/lpac
```

如果该构建还附带动态库，应一并放入 `/opt/simadmin/lpac/lib/`。普通 SIM、蜂窝网络和不依赖
eSIM 管理的功能不要求安装 `lpac`。

## 7. 访问管理后台

服务正常启动后访问：

```text
http://设备IP:3000
```

SimAdmin 没有默认初始密码。首次访问会进入管理员密码设置页面：

- 默认要求 8–64 个字符。
- 只能使用英文字母、数字和符号，不允许空格或中文。
- 至少包含两类字符。

忘记密码时，可通过 SSH 执行：

```bash
/opt/simadmin/simadmin auth reset-password
```

如需清除密码并重新进入首次设置：

```bash
/opt/simadmin/simadmin auth clear
```

## 8. 手动升级

当前不要使用 OTA 或在线升级。升级前先停止服务并备份用户数据：

```bash
systemctl stop simadmin.service
install -d -m 0700 /opt/simadmin/manual-backup
cp -a /opt/simadmin/data.db* /opt/simadmin/manual-backup/
cp -a /opt/simadmin/config.sqlite3* /opt/simadmin/manual-backup/ 2>/dev/null || true
cp -a /data/config.sqlite3* /opt/simadmin/manual-backup/ 2>/dev/null || true
cp -a /opt/simadmin/config.json /opt/simadmin/manual-backup/ 2>/dev/null || true
cp -a /data/config.json /opt/simadmin/manual-backup/ 2>/dev/null || true
cp -a /data/simadmin/e911 /opt/simadmin/manual-backup/ 2>/dev/null || true
```

然后按第 3、4 节重新传输并覆盖后端、前端和经过审核的 catalog，最后启动并检查日志：

```bash
systemctl start simadmin.service
systemctl status simadmin --no-pager
journalctl -u simadmin -n 100 --no-pager
```

`data.db`、`config.sqlite3` 和 E911 secret state 是用户数据，不要用发布包中的同名文件覆盖。
复制 SQLite 文件前必须先停止服务，不能在 WAL 活跃时只复制主文件。catalog 是独立只读制品，
升级时需要同时验证 schema、config contract 与 sealed 状态。

## 9. 暂停使用的功能

在新的仓库地址、制品命名、签名/校验、catalog 分发和回滚契约完成前，以下入口均视为
不可用：

- `install_latest.sh` 一键安装或升级。
- `uninstall.sh` 自动卸载。
- `scripts/build.sh` 生成的旧式 OTA 发布流程。
- `scripts/pack-ota.sh`。
- Web 页面中的在线检查、在线下载、OTA 上传和应用功能。

相关文件暂时保留是为了后续重构和历史对照，不表示当前已获得发布支持。
