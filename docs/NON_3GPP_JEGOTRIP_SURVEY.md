# 无忧行（com.cmi.jegotrip）托管能力勘察

只做静态分析，不含抓包、不含账号凭据。目标：判断 SimAdmin 能否替代手机端无忧行，
实现"托管收短信 / 接电话（免费）/ 视情况发短信"。

## 1. 包结构事实

| 项 | 值 |
|---|---|
| APK | 132 MB，7989 entries |
| JS | `assets/index.android.bundle`，13.5 MB，未加密可读（React Native） |
| 原生库 | `lib/arm64-v8a/` 66 个 .so |
| 关键库 | `libidss-cn.so` 2.8 MB、`libmsc.so` 2.5 MB、`libmtc.so` 24 MB |

## 2. 分层结论

- **业务层是 HTTP**：bundle 内域名集中在 `*.jegotrip.com.cn`（cdn/cp/oss/atv/szsvcdn）、
  `toss.t.jegotrip.cn`，属于业务 API 与 CDN，不是信令。
- **信令不在 JS**：bundle 内 `sip:`/`SIP/2.0`/`INVITE`/`idss` 出现次数为 0。
- **信令在原生层**：
  - `libidss-cn.so`：`jego` 字符串 1530 次、`REGISTER` 1 次、无 SIP 方法名
    → 自研私有协议，不是标准 SIP。
  - `libmsc.so`：`DTLS` 141 次 → 媒体加密走 DTLS（SRTP 密钥协商）。
  - `libmtc.so`：`SRTP` 32、`rtp` 275、`amr-wb` 1、`Authorization` 8
    → 完整媒体栈（接近 WebRTC 裁剪版）。
- **JS 与原生桥**：`NativeModules.JegoTripRN`（51 处）+ `NativeEventEmitter`；
  托管/通话状态通过事件回传，命令通过 bridge 下发。

## 3. 能否复刻

| 能力 | 通道 | 可行性 | 说明 |
|---|---|---|---|
| 登录 / 号码绑定 / 套餐 | HTTP + JS | 高 | 明文 JS 可枚举接口；需用户账号 |
| 托管开关 / 短信列表 / 通话记录 | HTTP | 高 | 只读为主，风险低 |
| 收短信（实时推送） | 原生私有协议 | 中 | 需还原 `libidss-cn.so` 的注册与下行通道 |
| 接听电话（免费） | 私有信令 + DTLS/SRTP | 低 | 需还原 INVITE 等价流程与密钥协商 |
| 发短信 | HTTP 或私有 | 未知 | JS 内无 sendSms 字样，待确认 |

## 4. 分阶段计划（本模块）

1. **P1 勘察（本文件）**：已完成。结论是"控制面可做，媒体面需逆向"。
2. **P2 控制面**：实现 `non_3gpp_voice` 的 `jegotrip` provider，先做只读能力
   （托管状态、短信列表、通话记录），凭据由用户本地配置，不入日志、不入 API 响应。
3. **P3 可行性闸门**：解析 `libidss-cn.so` 的 bridge 参数与注册流程；
   若确认可复现下行短信通道，再做实时收短信；否则停在此处并明确告知。
4. **P4 语音**：仅在 P3 通过后评估。媒体面（DTLS/SRTP + AMR-WB）成本极高，
   且服务端可能只对官方客户端放行。

## 5. 边界

- 不使用、不存储用户账号密码明文；凭据只进本地配置并脱敏。
- 不实现自动登录、不代发短信、不做批量操作。
- 所有 provider 仍走既有 `ProviderRegistry` 契约，默认零注册。
