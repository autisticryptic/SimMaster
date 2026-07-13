export type DeviceStatusItem = {
  key: string
  label: string
  defaultEnabled: boolean
}

export type DeviceStatusGroup = {
  key: string
  label: string
  items: DeviceStatusItem[]
}

export const DEVICE_STATUS_GROUPS: DeviceStatusGroup[] = [
  {
    key: 'device',
    label: '设备概览',
    items: [
      { key: 'device_power', label: '设备在线/上电状态', defaultEnabled: true },
      { key: 'device_model', label: '设备型号/厂商', defaultEnabled: true },
      { key: 'system_version', label: '系统版本/架构', defaultEnabled: true },
      { key: 'uptime', label: '运行时长', defaultEnabled: true },
    ],
  },
  {
    key: 'sim',
    label: 'SIM/eSIM',
    items: [
      { key: 'work_mode', label: '当前工作模式', defaultEnabled: true },
      { key: 'sim_present', label: 'SIM 是否存在', defaultEnabled: true },
      { key: 'sim_operator', label: '运营商 MCC/MNC', defaultEnabled: true },
      { key: 'phone_number', label: '当前号码', defaultEnabled: false },
      { key: 'sim_identifiers', label: 'ICCID/IMSI 脱敏摘要', defaultEnabled: false },
    ],
  },
  {
    key: 'cellular',
    label: '蜂窝网络',
    items: [
      { key: 'cellular_registration', label: '注册状态', defaultEnabled: true },
      { key: 'cellular_operator', label: '当前运营商', defaultEnabled: true },
      { key: 'cellular_technology', label: '网络制式/技术偏好', defaultEnabled: true },
      { key: 'signal_strength', label: '信号强度', defaultEnabled: true },
      { key: 'data_connection', label: '数据连接状态', defaultEnabled: true },
      { key: 'airplane_mode', label: '飞行模式', defaultEnabled: true },
      { key: 'roaming', label: '漫游状态', defaultEnabled: true },
      { key: 'cell_summary', label: '小区摘要', defaultEnabled: false },
    ],
  },
  {
    key: 'wlan',
    label: 'WLAN',
    items: [
      { key: 'wlan_enabled', label: 'WLAN 可用/启用状态', defaultEnabled: true },
      { key: 'wlan_connected', label: 'WLAN 连接状态', defaultEnabled: true },
      { key: 'wlan_ssid', label: '当前 SSID', defaultEnabled: true },
      { key: 'wlan_ip', label: 'WLAN 网关/IP 摘要', defaultEnabled: false },
      { key: 'key_interfaces', label: '关键接口状态', defaultEnabled: true },
      { key: 'cellular_traffic', label: '蜂窝流量', defaultEnabled: true },
      { key: 'wifi_traffic', label: 'Wi-Fi 流量', defaultEnabled: false },
    ],
  },
  {
    key: 'connectivity',
    label: 'IP 与连通性',
    items: [
      { key: 'ipv4_connectivity', label: 'IPv4 连通性', defaultEnabled: true },
      { key: 'ipv6_connectivity', label: 'IPv6 连通性', defaultEnabled: true },
      { key: 'default_route', label: '默认出口接口', defaultEnabled: true },
      { key: 'default_ip', label: '默认出口 IP', defaultEnabled: true },
    ],
  },
  {
    key: 'resource',
    label: '系统资源',
    items: [
      { key: 'cpu_usage', label: 'CPU 使用率', defaultEnabled: true },
      { key: 'memory_usage', label: '内存使用率', defaultEnabled: true },
      { key: 'root_disk', label: '根分区可用空间', defaultEnabled: true },
      { key: 'top_temperatures', label: '双高温度', defaultEnabled: true },
    ],
  },
  {
    key: 'service',
    label: '服务状态',
    items: [
      { key: 'service_version', label: 'SimAdmin 服务/版本', defaultEnabled: true },
      { key: 'ddns_status', label: 'DDNS 状态', defaultEnabled: true },
      { key: 'ota_status', label: 'OTA 更新状态', defaultEnabled: true },
    ],
  },
  {
    key: 'forwarding',
    label: '转发状态',
    items: [
      { key: 'sms_forwarding_stats', label: '短信转发统计', defaultEnabled: true },
      { key: 'forwarding_channels', label: '转发通道数量', defaultEnabled: true },
      { key: 'forwarding_rules', label: '转发规则数量', defaultEnabled: true },
    ],
  },
  {
    key: 'stats',
    label: '通讯统计',
    items: [
      { key: 'sms_stats', label: '短信统计', defaultEnabled: true },
    ],
  },
  {
    key: 'security',
    label: '安全摘要',
    items: [
      { key: 'security_password', label: '密码保护状态', defaultEnabled: false },
      { key: 'security_session', label: '会话/空闲超时', defaultEnabled: false },
    ],
  },
]

export const DEVICE_STATUS_TEMPLATE_VARIABLES = [
  { label: '状态分类', token: '{{状态分类}}' },
  { label: '状态内容', token: '{{状态内容}}' },
  { label: '时间', token: '{{时间}}' },
]

export const DEFAULT_DEVICE_STATUS_TEMPLATE = '设备状态报告\n【{{状态分类}}】\n{{状态内容}}\n\n时间: {{时间}}'

export function defaultDeviceStatusItems() {
  return DEVICE_STATUS_GROUPS.flatMap((group) => group.items)
    .filter((item) => item.defaultEnabled)
    .map((item) => item.key)
}

export function allDeviceStatusItems() {
  return DEVICE_STATUS_GROUPS.flatMap((group) => group.items.map((item) => item.key))
}
