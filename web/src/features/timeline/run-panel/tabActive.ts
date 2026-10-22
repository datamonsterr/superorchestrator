import { createContext, useContext } from 'react'

/** True while the tab that renders this subtree is the selected one. Hidden tabs stay mounted but go quiet. */
export const TabActiveContext = createContext(true)
export const useTabActive = () => useContext(TabActiveContext)
