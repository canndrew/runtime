extern crate libc;
#[macro_use]
extern crate unwrap;
extern crate coroutine;
extern crate mio;

use std::os::unix::io::{IntoRawFd, FromRawFd, RawFd};
use std::io;
use std::marker::PhantomData;

use coroutine::asymmetric::{Coroutine, Handle};

pub trait Runtime {
    fn read(&mut self, fd: RawFd, buf: &mut [u8]) -> io::Result<usize>;
    fn write(&mut self, fd: RawFd, buf: &[u8]) -> io::Result<usize>;
}

pub struct Native;
impl Runtime for Native {
    fn read(&mut self, fd: RawFd, buf: &mut [u8]) -> io::Result<usize> {
        let res = unsafe {
            libc::read(fd, buf.as_mut_ptr() as *mut _, buf.len())
        };
        if res < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(res as usize)
    }

    fn write(&mut self, fd: RawFd, buf: &[u8]) -> io::Result<usize> {
        let res = unsafe {
            libc::write(fd, buf.as_ptr() as *const _, buf.len())
        };
        if res < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(res as usize)
    }
}

type ResumeResult<T> = Result<T, (RawFd, mio::EventSet)>;

pub struct ResumableFunctionRuntime<'a, T: Send + 'static> {
    coro: &'a mut Coroutine,
    _ph: PhantomData<T>,
}

impl<'a, T: Send> Runtime for ResumableFunctionRuntime<'a, T> {
    fn read(&mut self, fd: RawFd, buf: &mut [u8]) -> io::Result<usize> {
        loop {
            let res = unsafe {
                libc::read(fd, buf.as_mut_ptr() as *mut _, buf.len())
            };
            if res < 0 {
                let last_err = io::Error::last_os_error();
                if last_err.kind() == io::ErrorKind::WouldBlock {
                    let res: ResumeResult<T> = Err((fd, mio::EventSet::readable()));
                    let res = Box::new(res);
                    let res = Box::into_raw(res);
                    self.coro.yield_with(res as usize);
                    continue;
                }
                return Err(last_err);
            }
            else {
                return Ok(res as usize);
            }
        }
    }

    fn write(&mut self, fd: RawFd, buf: &[u8]) -> io::Result<usize> {
        loop {
            let res = unsafe {
                libc::write(fd, buf.as_ptr() as *const _, buf.len())
            };
            if res < 0 {
                let last_err = io::Error::last_os_error();
                if last_err.kind() == io::ErrorKind::WouldBlock {
                    let res: ResumeResult<T> = Err((fd, mio::EventSet::writable()));
                    let res = Box::new(res);
                    let res = Box::into_raw(res);
                    self.coro.yield_with(res as usize);
                    continue;
                }
                return Err(last_err);
            }
            else {
                return Ok(res as usize);
            }
        }
    }
}

pub struct ResumableFunction<T: Send + 'static> {
    handle: Handle,
    blocked_fd: RawFd,
    blocked_event: mio::EventSet,
    _ph: PhantomData<T>,
}

impl<T: Send> ResumableFunction<T> {
    /// Create a resumable function. If the function can run to completion immediately then its
    /// result is returned. Otherwise a `ResumableFunction` is returned.
    pub fn new<F: for<'a> FnOnce(ResumableFunctionRuntime<'a, T>) -> T + Send + 'static>(f: F) -> Result<T, ResumableFunction<T>> {
        let mut handle = Coroutine::spawn(|me| {
            let res: ResumeResult<T> = {
                let rt = ResumableFunctionRuntime {
                    coro: me,
                    _ph: PhantomData,
                };
                let res = f(rt);
                Ok(res)
            };
            let res = Box::new(res);
            let res = Box::into_raw(res);
            me.yield_with(res as usize);
        });
        let ptr = unwrap!(handle.next()) as *mut ResumeResult<T>;
        let ptr = unsafe { Box::from_raw(ptr) };
        match *ptr {
            Ok(res) => Ok(res),
            Err((fd, event)) => {
                Err(ResumableFunction {
                    handle: handle,
                    blocked_fd: fd,
                    blocked_event: event,
                    _ph: PhantomData,
                })
            },
        }
    }

    /// Resume the function. Returns `Some(t)` if it was able to run to completion. Otherwise the
    /// function must be reregistered with the event loop.
    pub fn resume(&mut self) -> Option<T> {
        let ptr = unwrap!(self.handle.next()) as *mut ResumeResult<T>;
        let ptr = unsafe { Box::from_raw(ptr) };
        match *ptr {
            Ok(t) => Some(t),
            Err((fd, event)) => {
                self.blocked_fd = fd;
                self.blocked_event = event;
                None
            },
        }
    }

    /// Register the machine with a mio event loop.
    pub fn register(&mut self, poll: &mut mio::Poll, token: mio::Token) -> io::Result<()> {
        let file = unsafe { mio::tcp::TcpStream::from_raw_fd(self.blocked_fd) };
        let res = poll.register(&file, token, self.blocked_event, mio::PollOpt::edge());
        let _ = file.into_raw_fd();
        res
    }

    /// Reregister the machine with a mio event loop.
    pub fn reregister(&mut self, poll: &mut mio::Poll, token: mio::Token) -> io::Result<()> {
        let file = unsafe { mio::tcp::TcpStream::from_raw_fd(self.blocked_fd) };
        let res = poll.reregister(&file, token, self.blocked_event, mio::PollOpt::edge());
        let _ = file.into_raw_fd();
        res
    }
}

