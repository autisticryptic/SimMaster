# 派生注册离线模拟

在 SimAdmin 根目录运行：

```bash
python3 offline-registration-sim/run.py \
  --target-dir /path/to/cargo-cache \
  --report offline-registration-sim/results/new-report.json
```

脚本只执行真实代码的 `offline_derivation_registration_matrix` 测试，不启动服务、不操作410。
19 场景中12个完成模拟注册、7个按预期拒绝，包含421/494累加回退、407、AKA算法、423、
UDP原报文重传、乱序消息与错误凭据/普通403等反例。内存对端独立校验Digest后才返回200。

SIM返回材料和传输保护是测试替身，不执行无线、完整IKE/IPsec、NAS、5G-AKA或VoNR。
NR仅测试命名，不声称5G注册通过。报告绑定源文件摘要且明确 `live_network_verified=false`。

数据库项目的可移植副本位于 `../carrier_Bundles/simulations/ims_registration/`，其中运行器
会在临时副本注入测试入口，不改变调用方checkout。对应筛选器在
`../carrier_Bundles/simulation_pruning/`。当前使用原schema v7/contract v1，直接删除符合
已测试标准模型的LTE/VoWiFi接入配置，其余保留；**不再采用v2格式/重建规则**。

实库验证采用明确home PLMN及合成IMSI，618项删除接入都能通过现有来源绑定路径落到派生配置，
其他接入保持原投影。它证明解析与测试场景，不是618家运营商真实网络都已验收。
