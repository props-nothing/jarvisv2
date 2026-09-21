//! Native local client transports for the JARVIS daemon.
//!
//! Unix platforms use a Unix domain socket created with user-only access.
//! Windows uses a byte-mode named pipe that rejects remote clients and refuses
//! to become a second pipe instance. Filesystem locality is not treated as
//! authorization; authentication happens in the protocol handshake.

use std::io;

use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

use crate::LocalEndpoint;

/// Explains why a local transport could not be prepared or connected.
#[derive(Debug, thiserror::Error)]
pub enum TransportError {
    /// The caller asked for an endpoint this build did not produce.
    #[error("local endpoint kind does not match this platform transport")]
    EndpointKindMismatch,
    /// The transport listener could not be created.
    #[error("failed to create the local listener")]
    Create {
        /// The underlying operating-system error.
        #[source]
        source: io::Error,
    },
    /// The transport listener could not accept a connection.
    #[error("failed to accept a local client connection")]
    Accept {
        /// The underlying operating-system error.
        #[source]
        source: io::Error,
    },
    /// A client connection could not be opened.
    #[error("failed to connect to the local daemon endpoint")]
    Connect {
        /// The underlying operating-system error.
        #[source]
        source: io::Error,
    },
}

/// A bidirectional stream over the native local transport.
pub enum LocalStream {
    /// A Unix domain socket.
    #[cfg(unix)]
    Unix(tokio::net::UnixStream),
    /// The server end of a Windows named pipe.
    #[cfg(windows)]
    PipeServer(tokio::net::windows::named_pipe::NamedPipeServer),
    /// The client end of a Windows named pipe.
    #[cfg(windows)]
    PipeClient(tokio::net::windows::named_pipe::NamedPipeClient),
}

impl AsyncRead for LocalStream {
    fn poll_read(
        self: std::pin::Pin<&mut Self>,
        context: &mut std::task::Context<'_>,
        buffer: &mut ReadBuf<'_>,
    ) -> std::task::Poll<io::Result<()>> {
        match self.get_mut() {
            #[cfg(unix)]
            Self::Unix(stream) => std::pin::Pin::new(stream).poll_read(context, buffer),
            #[cfg(windows)]
            Self::PipeServer(stream) => std::pin::Pin::new(stream).poll_read(context, buffer),
            #[cfg(windows)]
            Self::PipeClient(stream) => std::pin::Pin::new(stream).poll_read(context, buffer),
        }
    }
}

impl AsyncWrite for LocalStream {
    fn poll_write(
        self: std::pin::Pin<&mut Self>,
        context: &mut std::task::Context<'_>,
        buffer: &[u8],
    ) -> std::task::Poll<io::Result<usize>> {
        match self.get_mut() {
            #[cfg(unix)]
            Self::Unix(stream) => std::pin::Pin::new(stream).poll_write(context, buffer),
            #[cfg(windows)]
            Self::PipeServer(stream) => std::pin::Pin::new(stream).poll_write(context, buffer),
            #[cfg(windows)]
            Self::PipeClient(stream) => std::pin::Pin::new(stream).poll_write(context, buffer),
        }
    }

    fn poll_flush(
        self: std::pin::Pin<&mut Self>,
        context: &mut std::task::Context<'_>,
    ) -> std::task::Poll<io::Result<()>> {
        match self.get_mut() {
            #[cfg(unix)]
            Self::Unix(stream) => std::pin::Pin::new(stream).poll_flush(context),
            #[cfg(windows)]
            Self::PipeServer(stream) => std::pin::Pin::new(stream).poll_flush(context),
            #[cfg(windows)]
            Self::PipeClient(stream) => std::pin::Pin::new(stream).poll_flush(context),
        }
    }

    fn poll_shutdown(
        self: std::pin::Pin<&mut Self>,
        context: &mut std::task::Context<'_>,
    ) -> std::task::Poll<io::Result<()>> {
        match self.get_mut() {
            #[cfg(unix)]
            Self::Unix(stream) => std::pin::Pin::new(stream).poll_shutdown(context),
            #[cfg(windows)]
            Self::PipeServer(stream) => std::pin::Pin::new(stream).poll_shutdown(context),
            #[cfg(windows)]
            Self::PipeClient(stream) => std::pin::Pin::new(stream).poll_shutdown(context),
        }
    }
}

/// A bound local listener that yields authenticated-ready byte streams.
pub enum LocalListener {
    /// A bound Unix domain socket listener.
    #[cfg(unix)]
    Unix {
        /// The bound listener.
        listener: tokio::net::UnixListener,
        /// The socket path removed when the listener is dropped.
        path: std::path::PathBuf,
    },
    /// A Windows named-pipe listener keeping one instance pending.
    #[cfg(windows)]
    Pipe {
        /// The full pipe name.
        name: String,
        /// The pre-created instance awaiting the next client.
        pending: Option<tokio::net::windows::named_pipe::NamedPipeServer>,
    },
}

impl LocalListener {
    /// Binds the native listener for a resolved endpoint.
    ///
    /// # Errors
    ///
    /// Returns [`TransportError`] when the endpoint kind does not match this
    /// platform or the listener cannot be created.
    pub fn bind(endpoint: &LocalEndpoint) -> Result<Self, TransportError> {
        match endpoint {
            #[cfg(unix)]
            LocalEndpoint::UnixSocket(path) => {
                let _ = std::fs::remove_file(path);
                let listener = tokio::net::UnixListener::bind(path)
                    .map_err(|source| TransportError::Create { source })?;
                harden_unix_socket(path).map_err(|source| TransportError::Create { source })?;
                Ok(Self::Unix {
                    listener,
                    path: path.clone(),
                })
            }
            #[cfg(windows)]
            LocalEndpoint::NamedPipe(name) => {
                let server = pipe_server(name, true)?;
                Ok(Self::Pipe {
                    name: name.clone(),
                    pending: Some(server),
                })
            }
            #[allow(unreachable_patterns)]
            _ => Err(TransportError::EndpointKindMismatch),
        }
    }

    /// Accepts the next client connection.
    ///
    /// # Errors
    ///
    /// Returns [`TransportError::Accept`] when the underlying accept fails.
    pub async fn accept(&mut self) -> Result<LocalStream, TransportError> {
        match self {
            #[cfg(unix)]
            Self::Unix { listener, .. } => {
                let (stream, _) = listener
                    .accept()
                    .await
                    .map_err(|source| TransportError::Accept { source })?;
                Ok(LocalStream::Unix(stream))
            }
            #[cfg(windows)]
            Self::Pipe { name, pending } => {
                // The next instance must be created only after this one has
                // connected. Two simultaneous listening instances of the same
                // pipe name let a client attach to the wrong one.
                let connected = pending.take().ok_or(TransportError::EndpointKindMismatch)?;
                connected
                    .connect()
                    .await
                    .map_err(|source| TransportError::Accept { source })?;
                *pending = Some(pipe_server(name, false)?);
                Ok(LocalStream::PipeServer(connected))
            }
            #[allow(unreachable_patterns)]
            _ => Err(TransportError::EndpointKindMismatch),
        }
    }
}

#[cfg(unix)]
impl Drop for LocalListener {
    fn drop(&mut self) {
        if let Self::Unix { path, .. } = self {
            let _ = std::fs::remove_file(path);
        }
    }
}

/// Opens a client connection to a resolved endpoint.
///
/// # Errors
///
/// Returns [`TransportError`] when the endpoint kind does not match this
/// platform or the connection fails.
pub async fn connect(endpoint: &LocalEndpoint) -> Result<LocalStream, TransportError> {
    match endpoint {
        #[cfg(unix)]
        LocalEndpoint::UnixSocket(path) => {
            let stream = tokio::net::UnixStream::connect(path)
                .await
                .map_err(|source| TransportError::Connect { source })?;
            Ok(LocalStream::Unix(stream))
        }
        #[cfg(windows)]
        LocalEndpoint::NamedPipe(name) => {
            use tokio::net::windows::named_pipe::ClientOptions;
            let client = ClientOptions::new()
                .open(name)
                .map_err(|source| TransportError::Connect { source })?;
            Ok(LocalStream::PipeClient(client))
        }
        #[allow(unreachable_patterns)]
        _ => Err(TransportError::EndpointKindMismatch),
    }
}

#[cfg(windows)]
fn pipe_server(
    name: &str,
    first: bool,
) -> Result<tokio::net::windows::named_pipe::NamedPipeServer, TransportError> {
    use tokio::net::windows::named_pipe::ServerOptions;
    let mut options = ServerOptions::new();
    options.reject_remote_clients(true);
    if first {
        // `FILE_FLAG_FIRST_PIPE_INSTANCE` fails when any instance of the name
        // already exists, so a second daemon cannot take over the profile pipe.
        // It must not be set on later instances created by this same daemon.
        options.first_pipe_instance(true);
    }
    options
        .create(name)
        .map_err(|source| TransportError::Create { source })
}

#[cfg(unix)]
fn harden_unix_socket(path: &std::path::Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
}
