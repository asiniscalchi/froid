const TOKEN_STORAGE_KEY = 'froid-auth-token'
const UNAUTHORIZED_EVENT = 'froid:unauthorized'

export class UnauthorizedError extends Error {
  constructor() {
    super('Authentication required')
    this.name = 'UnauthorizedError'
  }
}

export function getToken(): string | null {
  return window.localStorage.getItem(TOKEN_STORAGE_KEY)
}

export function setToken(token: string): void {
  window.localStorage.setItem(TOKEN_STORAGE_KEY, token)
}

export function clearToken(): void {
  window.localStorage.removeItem(TOKEN_STORAGE_KEY)
}

export function onUnauthorized(listener: () => void): () => void {
  window.addEventListener(UNAUTHORIZED_EVENT, listener)
  return () => window.removeEventListener(UNAUTHORIZED_EVENT, listener)
}

/**
 * `fetch` with the stored bearer token attached. A 401 clears nothing but
 * notifies `onUnauthorized` subscribers (the app shows the token gate) and
 * throws `UnauthorizedError`.
 */
export async function apiFetch(
  input: string,
  init: RequestInit = {},
): Promise<Response> {
  const headers: Record<string, string> = {
    ...(init.headers as Record<string, string> | undefined),
  }
  const token = getToken()
  if (token) {
    headers['Authorization'] = `Bearer ${token}`
  }
  const response = await fetch(input, { ...init, headers })
  if (response.status === 401) {
    window.dispatchEvent(new Event(UNAUTHORIZED_EVENT))
    throw new UnauthorizedError()
  }
  return response
}
