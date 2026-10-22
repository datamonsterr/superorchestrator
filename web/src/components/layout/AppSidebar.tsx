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
  useSidebar,
} from '@/components/ui/sidebar'
import { NAV_ITEMS, type NavItem } from './nav'

/** The desktop container takes the landmark props; on mobile the sheet is a labelled dialog. */
export function AppSidebar() {
  return (
    <Sidebar collapsible="icon" role="complementary" aria-label="Sidebar">
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
    </Sidebar>
  )
}

function NavItemButton({ to, label, icon: Icon }: NavItem) {
  const active = useMatch({ path: to, end: false }) !== null
  const { setOpenMobile } = useSidebar()
  return (
    <SidebarMenuButton asChild isActive={active} tooltip={label}>
      <NavLink to={to} onClick={() => setOpenMobile(false)}>
        <Icon aria-hidden />
        <span>{label}</span>
      </NavLink>
    </SidebarMenuButton>
  )
}
