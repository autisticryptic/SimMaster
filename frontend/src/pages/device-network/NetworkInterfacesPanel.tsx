import { useMemo, useState } from 'react'
import {
  Box,
  Card,
  CardContent,
  Chip,
  FormControlLabel,
  Stack,
  Switch,
  Typography,
} from '@mui/material'
import { Lan, Router } from '@mui/icons-material'
import type { NetworkInterfaceInfo } from '../../api/current'

type Props = {
  interfaces: NetworkInterfaceInfo[]
}

function formatBytes(bytes: number) {
  if (!Number.isFinite(bytes) || bytes <= 0) return '0 B'
  const units = ['B', 'KB', 'MB', 'GB', 'TB']
  const index = Math.min(Math.floor(Math.log(bytes) / Math.log(1024)), units.length - 1)
  return `${(bytes / 1024 ** index).toFixed(index === 0 ? 0 : 1)} ${units[index]}`
}

function interfaceKind(item: NetworkInterfaceInfo) {
  if (item.is_cellular) return '蜂窝'
  if (item.is_wireless) return '无线'
  return '有线/虚拟'
}

export default function NetworkInterfacesPanel({ interfaces }: Props) {
  const [showDown, setShowDown] = useState(false)
  const [showAddresses, setShowAddresses] = useState(true)
  const filtered = useMemo(
    () => showDown ? interfaces : interfaces.filter((item) => item.status.toLowerCase() !== 'down'),
    [interfaces, showDown],
  )

  return (
    <Stack spacing={2}>
      <Box display="flex" alignItems="center" gap={1.5} flexWrap="wrap">
        <Router color="primary" />
        <Box minWidth={0} flexGrow={1}>
          <Typography variant="subtitle1" fontWeight={700}>网络接口</Typography>
          <Typography variant="body2" color="text.secondary">查看设备全部物理、蜂窝与虚拟网络接口</Typography>
        </Box>
        <Chip size="small" color="primary" variant="outlined" label={`${filtered.length} / ${interfaces.length}`} />
        <FormControlLabel control={<Switch size="small" checked={showDown} onChange={(_, value) => setShowDown(value)} />} label="显示离线接口" />
        <FormControlLabel control={<Switch size="small" checked={showAddresses} onChange={(_, value) => setShowAddresses(value)} />} label="显示 IP" />
      </Box>

      <Box display="grid" gridTemplateColumns={{ xs: '1fr', md: 'repeat(2, minmax(0, 1fr))' }} gap={2}>
        {filtered.map((item) => {
          const online = item.status.toLowerCase() !== 'down'
          return (
            <Card key={item.name} variant="outlined">
              <CardContent>
                <Box display="flex" alignItems="flex-start" gap={1.25}>
                  <Lan color={online ? 'primary' : 'disabled'} />
                  <Box minWidth={0} flexGrow={1}>
                    <Typography variant="subtitle2" fontFamily="monospace" sx={{ wordBreak: 'break-all' }}>{item.name}</Typography>
                    <Stack direction="row" spacing={0.75} mt={0.75} flexWrap="wrap" useFlexGap>
                      <Chip size="small" label={online ? '在线' : '离线'} color={online ? 'success' : 'default'} variant="outlined" />
                      <Chip size="small" label={interfaceKind(item)} variant="outlined" />
                      {item.is_default_ipv4 && <Chip size="small" label="默认 IPv4" color="info" variant="outlined" />}
                      {item.is_default_ipv6 && <Chip size="small" label="默认 IPv6" color="info" variant="outlined" />}
                    </Stack>
                  </Box>
                </Box>

                {showAddresses && (
                  <Box mt={2}>
                    <Typography variant="caption" color="text.secondary">IP 地址</Typography>
                    {item.ip_addresses.length > 0 ? item.ip_addresses.map((address) => (
                      <Typography key={`${address.address}/${address.prefix_len}`} variant="body2" fontFamily="monospace" sx={{ mt: 0.25, wordBreak: 'break-all' }}>
                        {address.address}/{address.prefix_len}
                      </Typography>
                    )) : <Typography variant="body2" color="text.secondary">未分配</Typography>}
                  </Box>
                )}

                <Box display="grid" gridTemplateColumns="repeat(2, minmax(0, 1fr))" gap={1.5} mt={2}>
                  <Box><Typography variant="caption" color="text.secondary">接收</Typography><Typography variant="body2">{formatBytes(item.rx_bytes)} · {item.rx_packets.toLocaleString()} 包</Typography></Box>
                  <Box><Typography variant="caption" color="text.secondary">发送</Typography><Typography variant="body2">{formatBytes(item.tx_bytes)} · {item.tx_packets.toLocaleString()} 包</Typography></Box>
                  <Box><Typography variant="caption" color="text.secondary">MAC / MTU</Typography><Typography variant="body2" sx={{ wordBreak: 'break-all' }}>{item.mac_address || '无'} · {item.mtu}</Typography></Box>
                  <Box><Typography variant="caption" color="text.secondary">错误</Typography><Typography variant="body2">RX {item.rx_errors} · TX {item.tx_errors}</Typography></Box>
                </Box>
              </CardContent>
            </Card>
          )
        })}
      </Box>
      {filtered.length === 0 && <Typography color="text.secondary" textAlign="center" py={6}>没有符合筛选条件的网络接口</Typography>}
    </Stack>
  )
}
