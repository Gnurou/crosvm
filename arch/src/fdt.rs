// Copyright 2018 The Chromium OS Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use byteorder::{BigEndian, ByteOrder};
use libc::{c_char, c_int, c_void};
use std::error::{self, Error as FdtError};
use std::ffi::{CStr, CString};
use std::fmt;
use std::ptr::null;

// This links to libfdt which handles the creation of the binary blob
// flattened device tree (fdt) that is passed to the kernel and indicates
// the hardware configuration of the machine.
#[link(name = "fdt")]
extern "C" {
    fn fdt_create(buf: *mut c_void, bufsize: c_int) -> c_int;
    fn fdt_finish_reservemap(fdt: *mut c_void) -> c_int;
    fn fdt_begin_node(fdt: *mut c_void, name: *const c_char) -> c_int;
    fn fdt_property(fdt: *mut c_void, name: *const c_char, val: *const c_void, len: c_int)
        -> c_int;
    fn fdt_end_node(fdt: *mut c_void) -> c_int;
    fn fdt_open_into(fdt: *const c_void, buf: *mut c_void, bufsize: c_int) -> c_int;
    fn fdt_finish(fdt: *const c_void) -> c_int;
    fn fdt_pack(fdt: *mut c_void) -> c_int;
}

#[derive(Debug)]
pub enum Error {
    FdtCreateError(c_int),
    FdtFinishReservemapError(c_int),
    FdtBeginNodeError(c_int),
    FdtPropertyError(c_int),
    FdtEndNodeError(c_int),
    FdtOpenIntoError(c_int),
    FdtFinishError(c_int),
    FdtPackError(c_int),
    FdtGuestMemoryWriteError,
}

impl error::Error for Error {
    fn description(&self) -> &str {
        match self {
            &Error::FdtCreateError(_) => "Error creating FDT",
            &Error::FdtFinishReservemapError(_) => "Error finishing reserve map",
            &Error::FdtBeginNodeError(_) => "Error beginning FDT node",
            &Error::FdtPropertyError(_) => "Error adding FDT property",
            &Error::FdtEndNodeError(_) => "Error ending FDT node",
            &Error::FdtOpenIntoError(_) => "Error copying FDT to Guest",
            &Error::FdtFinishError(_) => "Error performing FDT finish",
            &Error::FdtPackError(_) => "Error packing FDT",
            &Error::FdtGuestMemoryWriteError => "Error writing FDT to Guest Memory",
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let prefix = "Libfdt Error: ";
        match self {
            &Error::FdtCreateError(fdt_ret)
            | &Error::FdtFinishReservemapError(fdt_ret)
            | &Error::FdtBeginNodeError(fdt_ret)
            | &Error::FdtPropertyError(fdt_ret)
            | &Error::FdtEndNodeError(fdt_ret)
            | &Error::FdtOpenIntoError(fdt_ret)
            | &Error::FdtFinishError(fdt_ret)
            | &Error::FdtPackError(fdt_ret) => write!(
                f,
                "{} {} code: {}",
                prefix,
                Error::description(self),
                fdt_ret
            ),
            &Error::FdtGuestMemoryWriteError => {
                write!(f, "{} {}", prefix, Error::description(self))
            }
        }
    }
}

pub fn begin_node(fdt: &mut Vec<u8>, name: &str) -> Result<(), Box<Error>> {
    let cstr_name = CString::new(name).unwrap();

    // Safe because we allocated fdt and converted name to a CString
    let fdt_ret = unsafe { fdt_begin_node(fdt.as_mut_ptr() as *mut c_void, cstr_name.as_ptr()) };
    if fdt_ret != 0 {
        return Err(Box::new(Error::FdtBeginNodeError(fdt_ret)));
    }
    Ok(())
}

pub fn end_node(fdt: &mut Vec<u8>) -> Result<(), Box<Error>> {
    // Safe because we allocated fdt
    let fdt_ret = unsafe { fdt_end_node(fdt.as_mut_ptr() as *mut c_void) };
    if fdt_ret != 0 {
        return Err(Box::new(Error::FdtEndNodeError(fdt_ret)));
    }
    Ok(())
}

pub fn property(fdt: &mut Vec<u8>, name: &str, val: &[u8]) -> Result<(), Box<Error>> {
    let cstr_name = CString::new(name).unwrap();
    let val_ptr = val.as_ptr() as *const c_void;

    // Safe because we allocated fdt and converted name to a CString
    let fdt_ret = unsafe {
        fdt_property(
            fdt.as_mut_ptr() as *mut c_void,
            cstr_name.as_ptr(),
            val_ptr,
            val.len() as i32,
        )
    };
    if fdt_ret != 0 {
        return Err(Box::new(Error::FdtPropertyError(fdt_ret)));
    }
    Ok(())
}

fn cpu_to_fdt32(input: u32) -> [u8; 4] {
    let mut buf = [0; 4];
    BigEndian::write_u32(&mut buf, input);
    buf
}

fn cpu_to_fdt64(input: u64) -> [u8; 8] {
    let mut buf = [0; 8];
    BigEndian::write_u64(&mut buf, input);
    buf
}

pub fn property_u32(fdt: &mut Vec<u8>, name: &str, val: u32) -> Result<(), Box<Error>> {
    property(fdt, name, &cpu_to_fdt32(val))
}

pub fn property_u64(fdt: &mut Vec<u8>, name: &str, val: u64) -> Result<(), Box<Error>> {
    property(fdt, name, &cpu_to_fdt64(val))
}

// Helper to generate a properly formatted byte vector using 32-bit cells
pub fn generate_prop32(cells: &[u32]) -> Vec<u8> {
    let mut ret: Vec<u8> = Vec::new();
    for &e in cells {
        ret.extend(cpu_to_fdt32(e).into_iter());
    }
    ret
}

// Helper to generate a properly formatted byte vector using 64-bit cells
pub fn generate_prop64(cells: &[u64]) -> Vec<u8> {
    let mut ret: Vec<u8> = Vec::new();
    for &e in cells {
        ret.extend(cpu_to_fdt64(e).into_iter());
    }
    ret
}

pub fn property_null(fdt: &mut Vec<u8>, name: &str) -> Result<(), Box<Error>> {
    let cstr_name = CString::new(name).unwrap();

    // Safe because we allocated fdt, converted name to a CString
    let fdt_ret = unsafe {
        fdt_property(
            fdt.as_mut_ptr() as *mut c_void,
            cstr_name.as_ptr(),
            null(),
            0,
        )
    };
    if fdt_ret != 0 {
        return Err(Box::new(Error::FdtPropertyError(fdt_ret)));
    }
    Ok(())
}

pub fn property_cstring(
    fdt: &mut Vec<u8>,
    name: &str,
    cstr_value: &CStr,
) -> Result<(), Box<Error>> {
    let value_bytes = cstr_value.to_bytes_with_nul();
    let cstr_name = CString::new(name).unwrap();

    // Safe because we allocated fdt, converted name and value to CStrings
    let fdt_ret = unsafe {
        fdt_property(
            fdt.as_mut_ptr() as *mut c_void,
            cstr_name.as_ptr(),
            value_bytes.as_ptr() as *mut c_void,
            value_bytes.len() as i32,
        )
    };
    if fdt_ret != 0 {
        return Err(Box::new(Error::FdtPropertyError(fdt_ret)));
    }
    Ok(())
}

pub fn property_string(fdt: &mut Vec<u8>, name: &str, value: &str) -> Result<(), Box<Error>> {
    let cstr_value = CString::new(value).unwrap();
    property_cstring(fdt, name, &cstr_value)
}

pub fn start_fdt(fdt: &mut Vec<u8>, fdt_max_size: usize) -> Result<(), Box<Error>> {
    // Safe since we allocated this array with fdt_max_size
    let mut fdt_ret = unsafe { fdt_create(fdt.as_mut_ptr() as *mut c_void, fdt_max_size as c_int) };

    if fdt_ret != 0 {
        return Err(Box::new(Error::FdtCreateError(fdt_ret)));
    }
    // Safe since we allocated this array
    fdt_ret = unsafe { fdt_finish_reservemap(fdt.as_mut_ptr() as *mut c_void) };
    if fdt_ret != 0 {
        return Err(Box::new(Error::FdtFinishReservemapError(fdt_ret)));
    }
    Ok(())
}

pub fn finish_fdt(
    fdt: &mut Vec<u8>,
    fdt_final: &mut Vec<u8>,
    fdt_max_size: usize,
) -> Result<(), Box<Error>> {
    // Safe since we allocated fdt_final and previously passed in it's size
    let mut fdt_ret = unsafe { fdt_finish(fdt.as_mut_ptr() as *mut c_void) };
    if fdt_ret != 0 {
        return Err(Box::new(Error::FdtFinishError(fdt_ret)));
    }

    // Safe because we allocated both arrays with the correct size
    fdt_ret = unsafe {
        fdt_open_into(
            fdt.as_mut_ptr() as *mut c_void,
            fdt_final.as_mut_ptr() as *mut c_void,
            fdt_max_size as i32,
        )
    };
    if fdt_ret != 0 {
        return Err(Box::new(Error::FdtOpenIntoError(fdt_ret)));
    }

    // Safe since we allocated fdt_final
    fdt_ret = unsafe { fdt_pack(fdt_final.as_mut_ptr() as *mut c_void) };
    if fdt_ret != 0 {
        return Err(Box::new(Error::FdtPackError(fdt_ret)));
    }
    Ok(())
}