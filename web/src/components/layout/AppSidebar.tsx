import { NavLink, useMatch } from 'react-router'
import {
  Sidebar,
  SidebarContent,
  SidebarGroup,
  SidebarGroupContent,
  SidebarHeader,
  SidebarMenu,
  SidebarMenuButton,
  SidebarMenuItem,
  SidebarRail,
} from '@/components/ui/sidebar'
import { NAV_ITEMS, type NavItem } from './nav'

export function AppSidebar() {
  return (
    <Sidebar collapsible="icon">
      {/* A landmark, so the brand and rail are not loose page content. */}
      <div role="complementary" aria-label="Sidebar" className="flex size-full min-h-0 flex-col">
        <SidebarHeader>
          <span className="truncate px-2 py-1 font-semibold group-data-[collapsible=icon]:hidden">Superorchestrator</span>
        </SidebarHeader>
        <SidebarContent>
          <SidebarGroup>
            <SidebarGroupContent>
              <nav aria-label="Main">
                <SidebarMenu>
                  {NAV_ITEMS.map((item) => (
                    <SidebarMenuItem key={item.to}>
                      <NavItemButton {...item} />
                    </SidebarMenuItem>
                  ))}
                </SidebarMenu>
              </nav>
            </SidebarGroupContent>
          </SidebarGroup>
        </SidebarContent>
        <SidebarRail />
      </div>
    </Sidebar>
  )
}

function NavItemButton({ to, label, icon: Icon }: NavItem) {
  const active = useMatch({ path: to, end: false }) !== null
  return (
    <SidebarMenuButton asChild isActive={active} tooltip={label}>
      <NavLink to={to}>
        <Icon aria-hidden />
        <span>{label}</span>
      </NavLink>
    </SidebarMenuButton>
  )
}
