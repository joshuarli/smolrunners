const DEFAULT_BASE = '';

export function apiBase() {
  if (typeof import.meta !== 'undefined' && import.meta.env?.VITE_API_BASE) {
    return import.meta.env.VITE_API_BASE.replace(/\/$/, '');
  }
  return DEFAULT_BASE;
}

export class ApiError extends Error {
  constructor(status, command, code, message) {
    super(message);
    this.name = 'ApiError';
    this.status = status;
    this.command = command;
    this.code = code;
  }
}

export async function apiRequest(method, path, body) {
  const response = await fetch(`${apiBase()}${path}`, {
    method,
    headers: { Accept: 'application/json', ...(body ? { 'Content-Type': 'application/json' } : {}) },
    body: body ? JSON.stringify(body) : undefined,
  });
  const text = await response.text();
  let reply;
  try {
    reply = JSON.parse(text);
  } catch {
    throw new ApiError(response.status, path, 'bad_json', `non-JSON response: ${text.slice(0, 200)}`);
  }
  if (!response.ok || reply.status >= 400) {
    throw new ApiError(
      reply.status ?? response.status,
      reply.command ?? path,
      reply.data?.code ?? 'request_failed',
      reply.data?.message ?? `${method} ${path} failed`,
    );
  }
  return reply;
}

export const api = {
  status: () => apiRequest('GET', '/api/v1/status'),
  hosts: () => apiRequest('GET', '/api/v1/hosts'),
  host: (id) => apiRequest('GET', `/api/v1/hosts/${id}`),
  profiles: () => apiRequest('GET', '/api/v1/profiles'),
  profile: (name) => apiRequest('GET', `/api/v1/profiles/${encodeURIComponent(name)}`),
  jobs: () => apiRequest('GET', '/api/v1/jobs'),
  report: () => apiRequest('GET', '/api/v1/jobs/report'),
  top: () => apiRequest('GET', '/api/v1/jobs/top'),
  job: (id) => apiRequest('GET', `/api/v1/jobs/${id}`),
  explain: (id) => apiRequest('GET', `/api/v1/jobs/${id}/explain`),
  metrics: (id) => apiRequest('GET', `/api/v1/jobs/${id}/metrics`),
};
