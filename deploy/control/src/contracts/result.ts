export const ResultCodes = {
  OK: 0,
  INVALID_INPUT: 2,
  PRECONDITION_FAILED: 3,
  RETRYABLE_DEPENDENCY: 4,
  UNCERTAIN_REMOTE_OUTCOME: 5,
  COMPATIBILITY_BLOCKED: 6,
  TERMINAL_FAILURE: 7,
  NOT_IMPLEMENTED: 8,
} as const;
export type ResultCode = keyof typeof ResultCodes;

export class ControlError extends Error {
  constructor(readonly code: Exclude<ResultCode, 'OK'>) {
    super(code);
    this.name = 'ControlError';
  }
}

/** Never serialize thrown errors, causes, provider bodies, argv or arbitrary payloads. */
export function failureResult(error: unknown): { code: ResultCode; exit_code: number; retry: 'RECONCILE' | 'DO_NOT_RETRY' } {
  const code = error instanceof ControlError ? error.code : 'TERMINAL_FAILURE';
  return { code, exit_code: ResultCodes[code], retry: code === 'UNCERTAIN_REMOTE_OUTCOME' ? 'RECONCILE' : 'DO_NOT_RETRY' };
}

/** Total redaction is intentional. Callers log separately validated operational fields only. */
export const redact = (_untrusted: unknown): string => '[REDACTED]';
