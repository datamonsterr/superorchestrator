import { Activity, Bot, GitBranch, KeyRound, LayoutDashboard, Route, Settings, type LucideIcon } from 'lucide-react'

export interface NavItem {
  to: string
  label: string
  icon: LucideIcon
}

/** `Session timeline` is not a nav item: it opens from Sessions and the dashboard. */
export const NAV_ITEMS: NavItem[] = [
  { to: '/dashboard', label: 'Dashboard', icon: LayoutDashboard },
  { to: '/sessions', label: 'Sessions', icon: Activity },
  { to: '/accounts', label: 'Accounts', icon: KeyRound },
  { to: '/routing', label: 'Levels', icon: Route },
  { to: '/agents', label: 'Agents', icon: Bot },
  { to: '/workflows', label: 'Workflows', icon: GitBranch },
  { to: '/settings', label: 'Settings', icon: Settings },
]
