#[cfg(feature = "service-input")]
use crate::client;
use std::fmt;

#[cfg(feature = "dvc")]
mod dvc;
#[cfg(feature = "sangfor")]
mod sangfor;
#[cfg(feature = "svc")]
mod svc;

pub enum Error {
    NotReady,
    InvalidChannelName(String),
    VirtualChannel(u32),
    Crossbeam(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter) -> Result<(), fmt::Error> {
        match self {
            Self::NotReady => write!(f, "not ready"),
            Self::InvalidChannelName(e) => write!(f, "invalid channel name: {e}"),
            Self::VirtualChannel(e) => {
                let e = match e {
                    1 => "already initialized".into(),
                    2 => "not initialized".into(),
                    3 => "already connected".into(),
                    4 => "not connected".into(),
                    5 => "too many channels".into(),
                    6 => "bad channel".into(),
                    7 => "bad channel handle".into(),
                    8 => "no buffer".into(),
                    9 => "bad init handle".into(),
                    10 => "not open".into(),
                    11 => "bad proc".into(),
                    12 => "no memory".into(),
                    13 => "unknown channel name".into(),
                    14 => "already opened".into(),
                    15 => "not in virtual channel entry".into(),
                    16 => "null data".into(),
                    17 => "zero length".into(),
                    18 => "invalid instance".into(),
                    19 => "unsupprted version".into(),
                    20 => "initialization error".into(),
                    _ => format!("unknown error 0x{e:x?}"),
                };

                write!(f, "virtual channel error: {e}")
            }
            Self::Crossbeam(e) => write!(f, "internal error: {e}"),
        }
    }
}

impl From<crossbeam_channel::RecvError> for Error {
    fn from(e: crossbeam_channel::RecvError) -> Self {
        Self::Crossbeam(e.to_string())
    }
}

impl<T> From<crossbeam_channel::SendError<T>> for Error {
    fn from(e: crossbeam_channel::SendError<T>) -> Self {
        Self::Crossbeam(e.to_string())
    }
}

pub trait VirtualChannel {
    fn open(&mut self) -> Result<(), Error>;
    #[cfg(feature = "service-input")]
    fn client(&self) -> Option<&client::Client>;
    #[cfg(feature = "service-input")]
    fn client_mut(&mut self) -> Option<&mut client::Client>;
    fn terminate(&mut self) -> Result<(), Error>;

    #[cfg(feature = "service-input")]
    fn reset_client(&mut self) {
        if let Some(client) = self.client_mut() {
            client.reset();
        }
    }
}

pub enum GenericChannel {
    #[cfg(feature = "dvc")]
    Dynamic(dvc::Dvc),
    #[cfg(feature = "svc")]
    Static(svc::Svc),
}

impl VirtualChannel for GenericChannel {
    fn open(&mut self) -> Result<(), Error> {
        match self {
            #[cfg(feature = "dvc")]
            Self::Dynamic(dvc) => dvc.open(),
            #[cfg(feature = "svc")]
            Self::Static(svc) => svc.open(),
        }
    }

    #[cfg(feature = "service-input")]
    fn client(&self) -> Option<&client::Client> {
        match self {
            #[cfg(feature = "dvc")]
            Self::Dynamic(dvc) => dvc.client(),
            #[cfg(feature = "svc")]
            Self::Static(svc) => svc.client(),
        }
    }

    #[cfg(feature = "service-input")]
    fn client_mut(&mut self) -> Option<&mut client::Client> {
        match self {
            #[cfg(feature = "dvc")]
            Self::Dynamic(dvc) => dvc.client_mut(),
            #[cfg(feature = "svc")]
            Self::Static(svc) => svc.client_mut(),
        }
    }

    fn terminate(&mut self) -> Result<(), Error> {
        match self {
            #[cfg(feature = "dvc")]
            Self::Dynamic(dvc) => dvc.terminate(),
            #[cfg(feature = "svc")]
            Self::Static(svc) => svc.terminate(),
        }
    }
}

pub trait Handle {
    fn write(&self, data: Vec<u8>) -> Result<(), Error>;
    fn close(&mut self) -> Result<(), Error>;
}

pub enum GenericHandle {
    #[cfg(feature = "dvc")]
    Dynamic(dvc::Handle),
    #[cfg(feature = "svc")]
    Static(svc::Handle),
}

impl Handle for GenericHandle {
    fn write(&self, data: Vec<u8>) -> Result<(), Error> {
        match self {
            #[cfg(feature = "dvc")]
            Self::Dynamic(dvc) => dvc.write(data),
            #[cfg(feature = "svc")]
            Self::Static(svc) => svc.write(data),
        }
    }

    fn close(&mut self) -> Result<(), Error> {
        match self {
            #[cfg(feature = "dvc")]
            Self::Dynamic(dvc) => dvc.close(),
            #[cfg(feature = "svc")]
            Self::Static(svc) => svc.close(),
        }
    }
}
