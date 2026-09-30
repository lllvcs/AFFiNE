import { OneKB } from '../../base';

export const ByteUnit = ['B', 'KB', 'MB', 'GB', 'TB', 'PB', 'EB', 'ZB', 'YB'];

/**
 * Mirrors the self-hosted ceilings in
 * `packages/backend/native/src/entitlement.rs`. Anything at or above these
 * values is rendered as "Unlimited" instead of a misleading number.
 */
export const UNLIMITED_STORAGE_QUOTA = 2 ** 53 - 1;
export const UNLIMITED_SEAT_LIMIT = 2 ** 31 - 1;
export const UNLIMITED_HISTORY_PERIOD = 100 * 365 * 24 * 60 * 60;

export function formatSize(bytes: number, decimals: number = 2): string {
  if (bytes === 0) return '0 B';
  if (bytes >= UNLIMITED_STORAGE_QUOTA) return 'Unlimited';

  const dm = decimals < 0 ? 0 : decimals;

  const i = Math.floor(Math.log(bytes) / Math.log(OneKB));

  return (
    parseFloat((bytes / Math.pow(OneKB, i)).toFixed(dm)) + ' ' + ByteUnit[i]
  );
}

export function formatDate(seconds: number): string {
  if (seconds >= UNLIMITED_HISTORY_PERIOD) return 'Unlimited';
  return `${(seconds / (24 * 60 * 60)).toFixed(0)} days`;
}

export function formatMemberLimit(limit: number): string {
  return limit >= UNLIMITED_SEAT_LIMIT ? 'Unlimited' : limit.toString();
}
