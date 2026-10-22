import { setupServer } from 'msw/node'
import { handlers } from '@/api/msw/handlers'

export const server = setupServer(...handlers)
