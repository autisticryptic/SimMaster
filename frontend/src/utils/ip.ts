import type { IpAddress } from '@/api/types'

export function publicIpv6Addresses(addresses: IpAddress[]): string[] {
  return addresses
    .filter((ip) => ip.ip_type === 'ipv6' && ip.scope === 'public')
    .sort((a, b) => Number(b.prefix_len === 128) - Number(a.prefix_len === 128))
    .map((ip) => ip.address)
}

export function publicIpv6AddressEntries(addresses: IpAddress[]): IpAddress[] {
  return addresses
    .filter((ip) => ip.ip_type === 'ipv6' && ip.scope === 'public')
    .sort((a, b) => Number(b.prefix_len === 128) - Number(a.prefix_len === 128))
}
