/**
 * Folders too broad to hand to ChatGPT without an explicit confirmation: a
 * drive or UNC share root, `C:\Users`, and a user profile folder directly
 * under it (the usual `%USERPROFILE%`). `homeDirectory`, when known, is also
 * treated as broad together with every folder that contains it.
 */
export function isBroadProjectPath(path: string, homeDirectory?: string | null): boolean {
  const candidate = normalize(path);
  if (!candidate) return false;
  if (/^[a-z]:$/.test(candidate)) return true;
  if (/^\\\\[^\\]+(\\[^\\]+)?$/.test(candidate)) return true;
  if (/^[a-z]:\\users(\\[^\\]+)?$/.test(candidate)) return true;
  const home = homeDirectory ? normalize(homeDirectory) : '';
  return Boolean(home) && (candidate === home || home.startsWith(`${candidate}\\`));
}

function normalize(path: string): string {
  let value = path.trim().replace(/\//g, '\\');
  if (value.startsWith('\\\\?\\UNC\\')) value = `\\\\${value.slice(8)}`;
  else if (value.startsWith('\\\\?\\')) value = value.slice(4);
  value = value.replace(/\\+$/, '');
  return value.toLowerCase();
}
