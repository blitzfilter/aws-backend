import { ControlError } from './result.js';

/** CalVer is a UTC minute, never local time. Tag creation/protection is a separate owner gate. */
export function parseCalVer(tag: string): Date {
  if (!/^[0-9]{8}-[0-9]{4}$/.test(tag) || tag.length !== 13) throw new ControlError('INVALID_INPUT');
  const year = Number(tag.slice(0, 4));
  const month = Number(tag.slice(4, 6));
  const day = Number(tag.slice(6, 8));
  const hour = Number(tag.slice(9, 11));
  const minute = Number(tag.slice(11, 13));
  if (year < 2000 || year > 9999) throw new ControlError('INVALID_INPUT');
  const date = new Date(Date.UTC(year, month - 1, day, hour, minute));
  if (date.getUTCFullYear() !== year || date.getUTCMonth() !== month - 1 || date.getUTCDate() !== day || date.getUTCHours() !== hour || date.getUTCMinutes() !== minute) throw new ControlError('INVALID_INPUT');
  return date;
}
