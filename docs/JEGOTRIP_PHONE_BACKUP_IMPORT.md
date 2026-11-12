# 手机成功托管备份 → SimAdmin：安全读取原 App UUID

本功能读取**本人手机已经存在的 App 身份材料**，不申请新设备授信、不恢复手机数据、不上传整个备份，也不把本地成功缓存当作服务器授权。生产托管引擎仍为 Rust；Python 只用于本地静态读取备份。

## 1. 支持的输入与尚不支持的封装

| 输入 | 处理 |
| --- | --- |
| 原始 `shared_prefs/setting_power.xml` | 只读取 SharedPreferences `map` 中唯一的 `string name="phone_uuid"`，保持 UUID 大小写与连字符 |
| 原始、可读的 `device.dat` | 仅接受内容本身就是 36 位 App UUID 的情况；未知编码/序列化不猜、不降级 |
| 普通 root 导出 ZIP | 离线脚本在内存中核对上述来源；来源冲突、不同导出根、路径穿越、符号链接、损坏 CRC 或超限立即停止，不解压到文件系统 |
| 脚本生成的 UUID-only JSON | 网页在本地解析，只接受 `version`、`source`、`app_device_uuid` 三个字段 |
| `SBA1` 数据或 `v1:::` 元数据 | 明确提示封装不可直接读取，不将其当成 XML、UUID 或认证 token；需要原工具导出解密后的原始数据 |

网页的 UUID-only 文件**不导入** SDK 的另一个 UUID，也不导入 `phone_safe_num`、缓存 `authentication_code`、旧 `enrollId`、App/运营商/电话 token 或 SIP 密码。快捷脚本会把独立 SDK UUID 另存私有核对文件，**不拿它代替 App UUID、不自动写入原生安装配置**。一份历史“托管成功”的备份不证明当前服务器仍认可原身份。

## 2. 一键从 root 手机读取（推荐）

将本人手机通过 USB 连接电脑，允许 USB 调试和已有 root 管理器的 shell 权限。在项目根执行：

```powershell
.\scripts\Export-JegotripTrust.ps1
```

只有一台已授权的 USB 手机时自动选中；多设备时明确选择，**不探测另一台设备**：

```powershell
.\scripts\Export-JegotripTrust.ps1 -Model 24122RKC7C
# 也可以用 -Serial 指定自己的设备，不要在公开报告中贴序列号。
```

- 使用已有 ADB、Python 和 `su`，不安装工具、不调用会重启 daemon 的 `adb root`。
- 默认只读取原 App UUID、独立 SDK UUID 和 App 版本，不复制全数据库/APK/日志，不读取账号密码或缓存 token 文件。
- 同一 App 和 SDK 身份配置都读取两遍并核对一致；变化或 root/包/格式/权限异常立即停止，不生成替代 UUID。App 数据目录、配置目录或文件是符号链接时拒绝，不沿 root 权限跟随。
- 自动在本项目 `.local/jegotrip/trusted-phone/<时间-随机后缀>/` 创建仅当前 Windows 用户和 SYSTEM 可访问的目录；不覆盖旧导出。只设置新目录的 DACL，不读取/修改审计 SACL，也不要求管理员审计权限；读取后、写入前再次核验私有路径与 ACL。
- `identity.json` 可直接用网页的本地文件入口；`sdk-context.private.json` 只供独立身份与版本核对，不含授信布尔值或登录凭据。
- `-WhatIf` 只显示将要做的操作，不查询手机；`-AdbPath` 可指定已安装的 ADB。
- 如果本机执行策略阻止自有脚本，可仅对本次进程使用 `powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\scripts\Export-JegotripTrust.ps1`，不更改系统执行策略。

手机已经离线时，也能从已保存的普通 root ZIP 生成同一格式，**不启动 ADB**：

```powershell
.\scripts\Export-JegotripTrust.ps1 -OfflineBackup .\.local\jegotrip\device-readonly-20261112\phone-root-export.zip
```

这提取的是已有身份材料，不是“生成/复制服务器授信证书”。离线 ZIP 中 App 与 SDK 身份必须来自同一个导出根，不能把另一份账号/设备的 SDK UUID 混进来。采集后仍用同本人账号/号码和原 UUID，按本次服务器回答继续复用。

## 2.1 手工准备原始 root 备份

在本人的 root 手机上，用已有文件管理器只读复制：

```text
/data/user/0/com.cmi.jegotrip/shared_prefs/
/data/user/0/com.cmi.jegotrip/files/
/data/user/0/com.cmi.jegotrip/databases/
```

按手机实际 Android 用户目录调整路径。两份账号配置分别保存，**不要混在一个导出 ZIP 中**。如果备份工具支持导出解密后的原始目录，也可以使用它。

- 保留原 App 数据和成功托管备份，不清缓存、不重装、不更改 UUID、不重新申请授信。
- 直接选择原始 `setting_power.xml` 即可使用网页入口；不需要把整个数据库或手机密码发到网页。
- 数据库及 SDK 文件仅作为后续本地核验材料，不是本入口的授权来源。
- 将私有压缩包放在项目根的 `无忧行*.zip` 或 `.local/` 内；这两类路径不会随 Git 发布。

## 3. 普通 ZIP 的离线读取

Windows PowerShell，在项目根运行现有 Python：

```powershell
python -B -X utf8 .\scripts\jegotrip_backup_identity.py ".\无忧行-原始配置.zip" --output ".\.local\jegotrip\phone-backup\identity.json"
```

不指定 `--output` 时只打印是否发现 UUID、匹配来源数量及文件 SHA256，**不打印 UUID 或任何 App 值**。指定输出时只允许写入本项目 `.local/` 下新的 JSON，不覆盖旧文件。

限额是本地资源策略，不是服务器协议上限：ZIP ≤32 MiB、最多 4096 成员、声明展开总量 ≤64 MiB，单个身份来源文件 ≤256 KiB。脚本只在内存读取选定成员并检查其 CRC，不恢复或执行备份。

## 4. 网页读取与确认

- 新登录：点击 **“从原始备份读取登录 UUID（仅本地）”**，选择原始 XML、可读 `device.dat` 或上述 JSON。只填入现有表单的 App UUID；默认选择 `reuse_hosting`，**清除授权勾选**。若原操作是只查询，则保留只查询、不启动接收。
- 已有身份：先选择原账号/号码的标识，再用 **“从原始备份读取待保存 UUID（仅本地）”**。读取不会自动保存；原版本 CAS、确认弹窗、占用检查和旧值所有权保护继续生效。
- 错误文件不会擦掉已有有效标识；取消读取、手工改值、切换目标或关闭表单后，较早的异步文件结果不能重新填回。
- 文件输入用完即清空，没有浏览器存储、后台备份上传或自动登录。

请核对该 UUID 所属的**同一本人账号与号码**。实际复用仍由本次 `getCallStatus.authentication_code=1`、业务读回、实时通话凭据及原生模板共同把关；历史文件不能授予托管或收件 ready。

## 5. 与原生通信工作的关系

本入口解决的是材料读取和防误导，不等于原生接收通路已经完整。新的显式 `offline_reception` Rust 通路、原集合/64位位置、事务消费和未知原包保留见 [原生离线积压](JEGOTRIP_NATIVE_OFFLINE_INBOX.md)；必须经过自身两Actions验证，不能借旧版本通过结果。MessageReceiver / ClientEx 的实际绑定、AccountAlive、完整媒体和真实收件时延仍须按实际证据完成。

编码、界面 fixture 与 Actions 回归不证明服务器授权或手机真实授信复用成功。完整验收仍需原始可读材料、当次真实服务器认可和明确操作条件。

参见 [最小配置与授信材料](JEGOTRIP_MINIMUM_CONFIGURATION.md) 和 [原生托管交付说明](JEGOTRIP_NATIVE_HOSTING_DELIVERY.md)。
