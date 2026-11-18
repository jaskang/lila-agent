/** An error whose message is meant for the user (Chinese, shown as-is); anything else is logged as a bug. */
export class UserError extends Error {}

export function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}
