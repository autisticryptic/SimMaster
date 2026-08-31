import { api, type CarrierProfileSummary } from '../../api/current'

let cachedProfiles: CarrierProfileSummary[] | null = null
let pendingLoad: Promise<CarrierProfileSummary[]> | null = null
let generation = 0

export function invalidateCarrierProfileSummaryCache() {
  generation += 1
  cachedProfiles = null
  pendingLoad = null
}

export function loadCarrierProfileSummaries(force = false): Promise<CarrierProfileSummary[]> {
  if (!force && cachedProfiles) return Promise.resolve(cachedProfiles)
  if (!force && pendingLoad) return pendingLoad

  const loadGeneration = generation
  const request = api.listVowifiCarrierProfiles().then((response) => {
    const profiles = response.data ?? []
    if (loadGeneration === generation) cachedProfiles = profiles
    return profiles
  }).finally(() => {
    if (pendingLoad === request) pendingLoad = null
  })
  pendingLoad = request
  return request
}
