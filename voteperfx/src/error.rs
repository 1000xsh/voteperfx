use thiserror::Error;

#[derive(Error, Debug)]
pub enum VoteMonitorError {
    #[error("grpc connection failed: {0}")]
    GrpcConnection(String),
    
    #[error("configuration error: {0}")]
    Config(String),
    
    #[error("file I/O error: {0}")]
    Io(#[from] std::io::Error),
    
    #[error("json serialization error: {0}")]
    Json(#[from] serde_json::Error),
    
    #[error("toml parsing error: {0}")]
    TomlDeserialization(#[from] toml::de::Error),
    
    #[error("toml serialization error: {0}")]
    TomlSerialization(#[from] toml::ser::Error),
    
    #[error("vote parsing error: {0}")]
    VoteParsing(String),
    
    #[error("dashboard rendering error: {0}")]
    Dashboard(String),
}

impl From<grpc_client::AppError> for VoteMonitorError {
    fn from(err: grpc_client::AppError) -> Self {
        VoteMonitorError::GrpcConnection(format!("{:?}", err))
    }
}

pub type Result<T> = std::result::Result<T, VoteMonitorError>;
