// Error payload decoding shared by synchronous core calls and worker jobs.
// Callers retain their own fallback for an absent payload.
export class CoreError extends Error {
  readonly kind: string;
  constructor(kind: string, message: string) {
    super(message);
    this.name = 'CoreError';
    this.kind = kind;
  }
}

/** Decode the core's string or externally tagged error kind. Malformed payloads
 * remain visible as the message rather than hiding the original failure. */
export function coreErrorFromJSON(json: string): CoreError {
  try {
    const obj = JSON.parse(json) as { error?: unknown; message?: string };
    let kind = 'internal';
    if (typeof obj.error === 'string') kind = obj.error;
    else if (obj.error && typeof obj.error === 'object') {
      kind = Object.keys(obj.error)[0] ?? 'internal';
    }
    return new CoreError(kind, obj.message ?? json);
  } catch {
    return new CoreError('internal', json);
  }
}
