use std::fs::{File, OpenOptions};
use std::io::Write;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::fs::OpenOptionsExt;
use std::os::windows::io::AsRawHandle;
use std::path::Path;

use windows_sys::Win32::Foundation::{CloseHandle, LocalFree, GENERIC_WRITE};
use windows_sys::Win32::Security::Authorization::{
    ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW, SetSecurityInfo,
    SDDL_REVISION_1, SE_FILE_OBJECT,
};
use windows_sys::Win32::Security::{
    GetSecurityDescriptorDacl, GetTokenInformation, TokenUser, DACL_SECURITY_INFORMATION,
    PROTECTED_DACL_SECURITY_INFORMATION, TOKEN_QUERY, TOKEN_USER,
};
use windows_sys::Win32::Storage::FileSystem::{
    GetFileInformationByHandle, MoveFileExW, BY_HANDLE_FILE_INFORMATION, FILE_ATTRIBUTE_DIRECTORY,
    FILE_ATTRIBUTE_REPARSE_POINT, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT,
    MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
};
use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

const READ_CONTROL: u32 = 0x0002_0000;
const WRITE_DAC: u32 = 0x0004_0000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct WindowsFileIdentity {
    pub(super) volume_serial: u32,
    pub(super) file_index: u64,
    pub(super) length: u64,
    pub(super) creation_time: u64,
    pub(super) last_write_time: u64,
}

pub(super) fn protect_private_directory(path: &Path) -> Result<(), String> {
    reject_reparse_path(path)?;
    let directory = OpenOptions::new()
        .access_mode(READ_CONTROL | WRITE_DAC)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)
        .map_err(|error| format!("could not open private Windows directory: {error}"))?;
    let info = file_information(&directory, "private Windows directory")?;
    if info.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY == 0 {
        return Err("private Windows state path is not a directory".to_string());
    }
    protect_handle(directory.as_raw_handle() as _, true)
}

pub(crate) fn write_new_private_file(path: &Path, content: &[u8]) -> Result<(), String> {
    reject_reparse_path(path)?;
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .access_mode(GENERIC_WRITE | READ_CONTROL | WRITE_DAC)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)
        .map_err(|error| format!("refused to overwrite existing private Windows state: {error}"))?;
    let result = (|| {
        let info = file_information(&file, "private Windows file")?;
        if info.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY != 0 {
            return Err("private Windows state path is not a regular file".to_string());
        }
        protect_handle(file.as_raw_handle() as _, false)?;
        file.write_all(content)
            .map_err(|error| format!("could not write private Windows state: {error}"))?;
        file.sync_all()
            .map_err(|error| format!("could not sync private Windows state: {error}"))
    })();
    drop(file);
    if result.is_err() {
        let _ = std::fs::remove_file(path);
    }
    result
}

pub(super) fn protect_private_file(path: &Path) -> Result<(), String> {
    reject_reparse_path(path)?;
    let file = OpenOptions::new()
        .access_mode(READ_CONTROL | WRITE_DAC)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)
        .map_err(|error| format!("could not open private Windows file: {error}"))?;
    let info = file_information(&file, "private Windows file")?;
    if info.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY != 0 {
        return Err("private Windows state path is not a regular file".to_string());
    }
    protect_handle(file.as_raw_handle() as _, false)
}

pub(super) fn replace_file(source: &Path, destination: &Path) -> Result<(), String> {
    file_identity(source, super::MAX_BINARY_BYTES)?;
    reject_reparse_path(destination)?;
    if destination
        .try_exists()
        .map_err(|error| error.to_string())?
    {
        // A corrupt cache may exceed the download limit; still allow recovery.
        file_identity(destination, u64::MAX)?;
    }
    let source: Vec<u16> = source
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let destination: Vec<u16> = destination
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    if unsafe {
        MoveFileExW(
            source.as_ptr(),
            destination.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    } == 0
    {
        return Err(format!(
            "could not atomically install the Windows tunnel client: {}",
            std::io::Error::last_os_error()
        ));
    }
    Ok(())
}

pub(super) fn file_identity(path: &Path, max_bytes: u64) -> Result<WindowsFileIdentity, String> {
    reject_reparse_path(path)?;
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)
        .map_err(|error| format!("could not open Windows tunnel client: {error}"))?;
    let info = file_information(&file, "Windows tunnel client")?;
    if info.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY != 0 {
        return Err("Windows tunnel client is not a regular file".to_string());
    }
    let length = ((info.nFileSizeHigh as u64) << 32) | info.nFileSizeLow as u64;
    if length > max_bytes {
        return Err("Windows tunnel client exceeds the safety limit".to_string());
    }
    Ok(WindowsFileIdentity {
        volume_serial: info.dwVolumeSerialNumber,
        file_index: ((info.nFileIndexHigh as u64) << 32) | info.nFileIndexLow as u64,
        length,
        creation_time: file_time(
            info.ftCreationTime.dwHighDateTime,
            info.ftCreationTime.dwLowDateTime,
        ),
        last_write_time: file_time(
            info.ftLastWriteTime.dwHighDateTime,
            info.ftLastWriteTime.dwLowDateTime,
        ),
    })
}

// OPEN_REPARSE_POINT only protects the final component of an open. Inspect
// existing ancestors as well, including junctions (which need no symlink privilege).
// Missing components are allowed for create_new/create_dir_all callers.
pub(super) fn reject_reparse_path(path: &Path) -> Result<(), String> {
    let absolute = std::path::absolute(path)
        .map_err(|error| format!("could not resolve Windows private path: {error}"))?;
    for component in absolute.ancestors() {
        match OpenOptions::new()
            .access_mode(READ_CONTROL)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
            .open(component)
        {
            Ok(file) => {
                file_information(&file, "Windows private path")?;
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(format!("could not inspect Windows private path: {error}")),
        }
    }
    Ok(())
}

fn file_information(file: &File, label: &str) -> Result<BY_HANDLE_FILE_INFORMATION, String> {
    let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
    if unsafe { GetFileInformationByHandle(file.as_raw_handle() as _, &mut info) } == 0 {
        return Err(format!(
            "could not inspect {label}: {}",
            std::io::Error::last_os_error()
        ));
    }
    if info.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        return Err(format!("refusing to use {label} through a reparse point"));
    }
    Ok(info)
}

fn file_time(high: u32, low: u32) -> u64 {
    ((high as u64) << 32) | low as u64
}

pub(super) fn current_user_sid() -> Result<String, String> {
    let mut token = std::ptr::null_mut();
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
        return Err("could not inspect the current Windows user identity".to_string());
    }
    let result = (|| {
        let mut required = 0u32;
        unsafe { GetTokenInformation(token, TokenUser, std::ptr::null_mut(), 0, &mut required) };
        if required == 0 {
            return Err("could not size the current Windows user identity".to_string());
        }
        let words = (required as usize).div_ceil(std::mem::size_of::<usize>());
        let mut buffer = vec![0usize; words];
        if unsafe {
            GetTokenInformation(
                token,
                TokenUser,
                buffer.as_mut_ptr().cast(),
                required,
                &mut required,
            )
        } == 0
        {
            return Err("could not read the current Windows user identity".to_string());
        }
        let token_user = unsafe { &*(buffer.as_ptr().cast::<TOKEN_USER>()) };
        let mut sid_text_ptr = std::ptr::null_mut();
        if unsafe { ConvertSidToStringSidW(token_user.User.Sid, &mut sid_text_ptr) } == 0 {
            return Err("could not encode the current Windows user identity".to_string());
        }
        let sid = unsafe {
            let mut len = 0usize;
            while *sid_text_ptr.add(len) != 0 {
                len += 1;
            }
            let value = String::from_utf16_lossy(std::slice::from_raw_parts(sid_text_ptr, len));
            LocalFree(sid_text_ptr as _);
            value
        };
        Ok(sid)
    })();
    unsafe { CloseHandle(token) };
    result
}

fn protect_handle(
    handle: windows_sys::Win32::Foundation::HANDLE,
    inherit_children: bool,
) -> Result<(), String> {
    let sid = current_user_sid()?;
    let ace_flags = if inherit_children { "OICI" } else { "" };
    let sddl = format!("D:P(A;{ace_flags};FA;;;{sid})(A;{ace_flags};FA;;;SY)");
    let sddl_wide: Vec<u16> = sddl.encode_utf16().chain(std::iter::once(0)).collect();
    let mut descriptor = std::ptr::null_mut();
    if unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            sddl_wide.as_ptr(),
            SDDL_REVISION_1,
            &mut descriptor,
            std::ptr::null_mut(),
        )
    } == 0
    {
        return Err("could not construct the protected Windows DACL".to_string());
    }
    let result = (|| {
        let mut present = 0;
        let mut defaulted = 0;
        let mut dacl = std::ptr::null_mut();
        if unsafe { GetSecurityDescriptorDacl(descriptor, &mut present, &mut dacl, &mut defaulted) }
            == 0
            || present == 0
            || dacl.is_null()
        {
            return Err("could not inspect the protected Windows DACL".to_string());
        }
        let status = unsafe {
            SetSecurityInfo(
                handle,
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                dacl,
                std::ptr::null_mut(),
            )
        };
        if status != 0 {
            return Err(format!(
                "could not install the protected Windows DACL: OS error {status}"
            ));
        }
        Ok(())
    })();
    unsafe { LocalFree(descriptor as _) };
    result
}
