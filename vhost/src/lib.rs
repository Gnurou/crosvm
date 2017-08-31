// Copyright 2017 The Chromium OS Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

extern crate libc;
extern crate net_util;
extern crate sys_util;
extern crate virtio_sys;

mod net;
pub use net::Net;

use std::mem;
use std::os::unix::io::AsRawFd;
use std::ptr::null;

use sys_util::{EventFd, GuestAddress, GuestMemory, GuestMemoryError, Error as SysError};
use sys_util::{ioctl, ioctl_with_ref, ioctl_with_mut_ref, ioctl_with_ptr};

#[derive(Debug)]
pub enum Error {
    /// Error opening vhost device.
    VhostOpen(SysError),
    /// Error while running ioctl.
    IoctlError(SysError),
    /// Invalid queue.
    InvalidQueue,
    /// Invalid descriptor table address.
    DescriptorTableAddress(GuestMemoryError),
    /// Invalid used address.
    UsedAddress(GuestMemoryError),
    /// Invalid available address.
    AvailAddress(GuestMemoryError),
    /// Invalid log address.
    LogAddress(GuestMemoryError),
}
pub type Result<T> = std::result::Result<T, Error>;

fn ioctl_result<T>() -> Result<T> {
    Err(Error::IoctlError(SysError::last()))
}

/// An interface for setting up vhost-based virtio devices.  Vhost-based devices are different
/// from regular virtio devices because the host kernel takes care of handling all the data
/// transfer.  The device itself only needs to deal with setting up the kernel driver and
/// managing the control channel.
pub trait Vhost: AsRawFd + std::marker::Sized {
    /// Get the guest memory mapping.
    fn mem(&self) -> &GuestMemory;

    /// Set the current process as the owner of this file descriptor.
    /// This must be run before any other vhost ioctls.
    fn set_owner(&self) -> Result<()> {
        // This ioctl is called on a valid vhost_net fd and has its
        // return value checked.
        let ret = unsafe { ioctl(self, virtio_sys::VHOST_SET_OWNER()) };
        if ret < 0 {
            return ioctl_result();
        }
        Ok(())
    }

    /// Get a bitmask of supported virtio/vhost features.
    fn get_features(&self) -> Result<u64> {
        let mut avail_features: u64 = 0;
        // This ioctl is called on a valid vhost_net fd and has its
        // return value checked.
        let ret = unsafe {
            ioctl_with_mut_ref(self,
                               virtio_sys::VHOST_GET_FEATURES(),
                               &mut avail_features)
        };
        if ret < 0 {
            return ioctl_result();
        }
        Ok(avail_features)
    }

    /// Inform the vhost subsystem which features to enable. This should be a subset of
    /// supported features from VHOST_GET_FEATURES.
    ///
    /// # Arguments
    /// * `features` - Bitmask of features to set.
    fn set_features(&self, features: u64) -> Result<()> {
        // This ioctl is called on a valid vhost_net fd and has its
        // return value checked.
        let ret =
            unsafe { ioctl_with_ref(self, virtio_sys::VHOST_SET_FEATURES(), &features) };
        if ret < 0 {
            return ioctl_result();
        }
        Ok(())
    }

    /// Set the guest memory mappings for vhost to use.
    fn set_mem_table(&self) -> Result<()> {
        let num_regions = self.mem().num_regions();
        let vec_size_bytes = mem::size_of::<virtio_sys::vhost_memory>() +
                             (num_regions * mem::size_of::<virtio_sys::vhost_memory_region>());
        let mut bytes: Vec<u8> = vec![0; vec_size_bytes];
        // Convert bytes pointer to a vhost_memory mut ref. The vector has been
        // sized correctly to ensure it can hold vhost_memory and N regions.
        let vhost_memory: &mut virtio_sys::vhost_memory =
            unsafe { &mut *(bytes.as_mut_ptr() as *mut virtio_sys::vhost_memory) };
        vhost_memory.nregions = num_regions as u32;
        // regions is a zero-length array, so taking a mut slice requires that
        // we correctly specify the size to match the amount of backing memory.
        let vhost_regions = unsafe { vhost_memory.regions.as_mut_slice(num_regions as usize) };

        let _ = self.mem()
                .with_regions_mut::<_, ()>(|index, guest_addr, size, host_addr| {
                    vhost_regions[index] = virtio_sys::vhost_memory_region {
                        guest_phys_addr: guest_addr.offset() as u64,
                        memory_size: size as u64,
                        userspace_addr: host_addr as u64,
                        flags_padding: 0u64,
                    };
                    Ok(())
                });


        // This ioctl is called with a pointer that is valid for the lifetime
        // of this function. The kernel will make its own copy of the memory
        // tables. As always, check the return value.
        let ret = unsafe {
            ioctl_with_ptr(self,
                           virtio_sys::VHOST_SET_MEM_TABLE(),
                           bytes.as_ptr())
        };
        if ret < 0 {
            return ioctl_result();
        }
        Ok(())
    }

    /// Set the number of descriptors in the vring.
    ///
    /// # Arguments
    /// * `queue_index` - Index of the queue to set descriptor count for.
    /// * `num` - Number of descriptors in the queue.
    fn set_vring_num(&self, queue_index: usize, num: u16) -> Result<()> {
        let vring_state = virtio_sys::vhost_vring_state {
            index: queue_index as u32,
            num: num as u32,
        };

        // This ioctl is called on a valid vhost_net fd and has its
        // return value checked.
        let ret = unsafe {
            ioctl_with_ref(self,
                           virtio_sys::VHOST_SET_VRING_NUM(),
                           &vring_state)
        };
        if ret < 0 {
            return ioctl_result();
        }
        Ok(())
    }

    // TODO(smbarber): This is copypasta. Eliminate the copypasta.
    fn is_valid(&self,
                queue_max_size: u16,
                queue_size: u16,
                desc_addr: GuestAddress,
                avail_addr: GuestAddress,
                used_addr: GuestAddress) -> bool {
        let desc_table_size = 16 * queue_size as usize;
        let avail_ring_size = 6 + 2 * queue_size as usize;
        let used_ring_size = 6 + 8 * queue_size as usize;
        if queue_size > queue_max_size || queue_size == 0 ||
                  (queue_size & (queue_size - 1)) != 0 {
            false
        } else if desc_addr
                      .checked_add(desc_table_size)
                      .map_or(true, |v| !self.mem().address_in_range(v)) {
            false
        } else if avail_addr
                      .checked_add(avail_ring_size)
                      .map_or(true, |v| !self.mem().address_in_range(v)) {
            false
        } else if used_addr
                      .checked_add(used_ring_size)
                      .map_or(true, |v| !self.mem().address_in_range(v)) {
            false
        } else {
            true
        }

    }

    /// Set the addresses for a given vring.
    ///
    /// # Arguments
    /// * `queue_max_size` - Maximum queue size supported by the device.
    /// * `queue_size` - Actual queue size negotiated by the driver.
    /// * `queue_index` - Index of the queue to set addresses for.
    /// * `flags` - Bitmask of vring flags.
    /// * `desc_addr` - Descriptor table address.
    /// * `used_addr` - Used ring buffer address.
    /// * `avail_addr` - Available ring buffer address.
    /// * `log_addr` - Optional address for logging.
    fn set_vring_addr(&self,
                      queue_max_size: u16,
                      queue_size: u16,
                      queue_index: usize,
                      flags: u32,
                      desc_addr: GuestAddress,
                      used_addr: GuestAddress,
                      avail_addr: GuestAddress,
                      log_addr: Option<GuestAddress>)
                      -> Result<()> {
        // TODO(smbarber): Refactor out virtio from crosvm so we can
        // validate a Queue struct directly.
        if !self.is_valid(queue_max_size, queue_size, desc_addr, used_addr, avail_addr) {
            return Err(Error::InvalidQueue);
        }

        let desc_addr = self.mem()
            .get_host_address(desc_addr)
            .map_err(Error::DescriptorTableAddress)?;
        let used_addr = self.mem()
            .get_host_address(used_addr)
            .map_err(Error::UsedAddress)?;
        let avail_addr = self.mem()
            .get_host_address(avail_addr)
            .map_err(Error::AvailAddress)?;
        let log_addr = match log_addr {
            None => null(),
            Some(a) => {
                self.mem()
                .get_host_address(a)
                .map_err(Error::LogAddress)?
            },
        };

        let vring_addr = virtio_sys::vhost_vring_addr {
            index: queue_index as u32,
            flags: flags,
            desc_user_addr: desc_addr as u64,
            used_user_addr: used_addr as u64,
            avail_user_addr: avail_addr as u64,
            log_guest_addr: log_addr as u64,
        };

        // This ioctl is called on a valid vhost_net fd and has its
        // return value checked.
        let ret = unsafe {
            ioctl_with_ref(self,
                           virtio_sys::VHOST_SET_VRING_ADDR(),
                           &vring_addr)
        };
        if ret < 0 {
            return ioctl_result();
        }
        Ok(())
    }

    /// Set the first index to look for available descriptors.
    ///
    /// # Arguments
    /// * `queue_index` - Index of the queue to modify.
    /// * `num` - Index where available descriptors start.
    fn set_vring_base(&self, queue_index: usize, num: u16) -> Result<()> {
        let vring_state = virtio_sys::vhost_vring_state {
            index: queue_index as u32,
            num: num as u32,
        };

        // This ioctl is called on a valid vhost_net fd and has its
        // return value checked.
        let ret = unsafe {
            ioctl_with_ref(self,
                           virtio_sys::VHOST_SET_VRING_BASE(),
                           &vring_state)
        };
        if ret < 0 {
            return ioctl_result();
        }
        Ok(())
    }

    /// Set the eventfd to trigger when buffers have been used by the host.
    ///
    /// # Arguments
    /// * `queue_index` - Index of the queue to modify.
    /// * `fd` - EventFd to trigger.
    fn set_vring_call(&self, queue_index: usize, fd: &EventFd) -> Result<()> {
        let vring_file = virtio_sys::vhost_vring_file {
            index: queue_index as u32,
            fd: fd.as_raw_fd(),
        };

        // This ioctl is called on a valid vhost_net fd and has its
        // return value checked.
        let ret = unsafe {
            ioctl_with_ref(self,
                           virtio_sys::VHOST_SET_VRING_CALL(),
                           &vring_file)
        };
        if ret < 0 {
            return ioctl_result();
        }
        Ok(())
    }

    /// Set the eventfd that will be signaled by the guest when buffers are
    /// available for the host to process.
    ///
    /// # Arguments
    /// * `queue_index` - Index of the queue to modify.
    /// * `fd` - EventFd that will be signaled from guest.
    fn set_vring_kick(&self, queue_index: usize, fd: &EventFd) -> Result<()> {
        let vring_file = virtio_sys::vhost_vring_file {
            index: queue_index as u32,
            fd: fd.as_raw_fd(),
        };

        // This ioctl is called on a valid vhost_net fd and has its
        // return value checked.
        let ret = unsafe {
            ioctl_with_ref(self,
                           virtio_sys::VHOST_SET_VRING_KICK(),
                           &vring_file)
        };
        if ret < 0 {
            return ioctl_result();
        }
        Ok(())
    }

}

#[cfg(test)]
mod tests {
    use super::*;

    use std::result;
    use sys_util::{GuestAddress, GuestMemory, GuestMemoryError};

    fn create_guest_memory() -> result::Result<GuestMemory, GuestMemoryError> {
        let start_addr1 = GuestAddress(0x0);
        let start_addr2 = GuestAddress(0x100);
        GuestMemory::new(&vec![(start_addr1, 0x100), (start_addr2, 0x400)])
    }

    #[test]
    fn open_vhostnet() {
        let gm = create_guest_memory().unwrap();
        Net::new(&gm).unwrap();
    }

    #[test]
    fn set_owner() {
        let gm = create_guest_memory().unwrap();
        let vhost_net = Net::new(&gm).unwrap();
        vhost_net.set_owner().unwrap();
    }

    #[test]
    fn get_features() {
        let gm = create_guest_memory().unwrap();
        let vhost_net = Net::new(&gm).unwrap();
        vhost_net.get_features().unwrap();
    }

    #[test]
    fn set_features() {
        let gm = create_guest_memory().unwrap();
        let vhost_net = Net::new(&gm).unwrap();
        vhost_net.set_features(0).unwrap();
    }
}