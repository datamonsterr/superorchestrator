export const ENDPOINT_PRESETS = ['none', 'zai', 'kimi', 'minimax', 'custom'] as const
export type EndpointPreset = (typeof ENDPOINT_PRESETS)[number]

interface PresetInfo {
  /** Full name, for selects and detail text. */
  label: string
  /** Short name, for table badges. */
  badge: string
  /** Default base URL; absent for `none` and `custom`. */
  url?: string
}

/** URLs mirror the built-in presets in crates/so-core/src/endpoint.rs: change both together. */
export const PRESET_INFO: Record<EndpointPreset, PresetInfo> = {
  none: { label: 'None (Anthropic subscription)', badge: 'None' },
  zai: { label: 'Z.ai GLM', badge: 'Z.ai', url: 'https://api.z.ai/api/anthropic' },
  kimi: { label: 'Kimi Code', badge: 'Kimi', url: 'https://api.kimi.ai/coding/' },
  minimax: { label: 'MiniMax', badge: 'MiniMax', url: 'https://api.minimax.io/anthropic' },
  custom: { label: 'Custom', badge: 'Custom' },
}

/** The API sends the preset as a plain string; unknown ones show as is. */
export function presetLabel(preset: string): string {
  return (PRESET_INFO as Record<string, PresetInfo | undefined>)[preset]?.label ?? preset
}
export function presetBadge(preset: string): string {
  return (PRESET_INFO as Record<string, PresetInfo | undefined>)[preset]?.badge ?? preset
}
