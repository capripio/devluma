import { z } from "zod";

/** Backend error codes. Keep in sync with `ErrorCode` in `src-tauri/src/error.rs`. */
export const BACKEND_ERROR_CODES = [
  "NETWORK_ERROR",
  "INTEGRITY_ERROR",
  "PERMISSION_DENIED",
  "PORT_IN_USE",
  "CONFIG_INVALID",
  "PROCESS_EXITED",
  "DISK_SPACE_LOW",
  "DATA_DIRECTORY_EXISTS",
  "RECOVERY_REQUIRED",
  "VALIDATION_ERROR",
  "NOT_FOUND",
  "NOT_IMPLEMENTED",
  "INTERNAL",
] as const;

/** `DESKTOP_UNAVAILABLE` is frontend-only: the UI is running outside the Tauri shell. */
export type ErrorCode = (typeof BACKEND_ERROR_CODES)[number] | "DESKTOP_UNAVAILABLE";

const ipcErrorSchema = z.object({
  code: z.enum(BACKEND_ERROR_CODES),
  message: z.string(),
  details: z.unknown().optional(),
  operationId: z.string().optional(),
  recoverable: z.boolean(),
});

export class AppError extends Error {
  readonly code: ErrorCode;
  readonly details?: unknown;
  readonly operationId?: string;
  readonly recoverable: boolean;

  constructor(init: {
    code: ErrorCode;
    message: string;
    details?: unknown;
    operationId?: string;
    recoverable: boolean;
  }) {
    super(init.message);
    this.name = "AppError";
    this.code = init.code;
    this.details = init.details;
    this.operationId = init.operationId;
    this.recoverable = init.recoverable;
  }
}

/**
 * Normalizes anything thrown across the IPC boundary into an `AppError`.
 * Unknown shapes become `INTERNAL` with a generic safe message; the raw value is not shown.
 */
export function toAppError(error: unknown): AppError {
  if (error instanceof AppError) return error;
  const parsed = ipcErrorSchema.safeParse(error);
  if (parsed.success) return new AppError(parsed.data);
  return new AppError({
    code: "INTERNAL",
    message: "Devluma received an unexpected error from its backend.",
    recoverable: true,
  });
}
