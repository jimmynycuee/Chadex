import { describe, expect, it } from 'vitest';
import { isBroadProjectPath } from '../src/projectPath';

describe('isBroadProjectPath', () => {
  it('flags drive roots, share roots, Users and user profiles', () => {
    for (const path of ['C:\\', 'c:', 'D:/', '\\\\?\\C:\\', 'C:\\Users', 'C:\\Users\\', 'C:\\Users\\alex', 'c:/users/Alex/', '\\\\server\\share', '\\\\?\\UNC\\server\\share\\']) {
      expect(isBroadProjectPath(path), path).toBe(true);
    }
  });
  it('flags the known home folder and every folder that contains it', () => {
    expect(isBroadProjectPath('E:\\Profiles\\alex', 'E:\\Profiles\\alex')).toBe(true);
    expect(isBroadProjectPath('E:\\Profiles', 'E:\\Profiles\\alex')).toBe(true);
    expect(isBroadProjectPath('E:\\Profiles\\alex\\code', 'E:\\Profiles\\alex')).toBe(false);
  });
  it('leaves project folders alone', () => {
    for (const path of ['C:\\Users\\alex\\Documents\\app', 'C:\\work\\project', 'D:\\src', '\\\\server\\share\\team\\app', '']) {
      expect(isBroadProjectPath(path), path).toBe(false);
    }
  });
});
