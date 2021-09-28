use crate::api::ApiService;
use crate::types::{Type, TypeSystem};
use chisel::chisel_rpc_server::{ChiselRpc, ChiselRpcServer};
use chisel::{
    StatusRequest, StatusResponse, TypeDefinitionRequest, TypeDefinitionResponse,
    TypeExportRequest, TypeExportResponse,
};
use convert_case::{Case, Casing};
use serde_json::json;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::sync::Mutex;
use tonic::{transport::Server, Request, Response, Status};

pub mod chisel {
    tonic::include_proto!("chisel");
}

/// RPC service for Chisel server.
///
/// The RPC service provides a Protobuf-based interface for Chisel control
/// plane. For example, the service has RPC calls for managing types and
/// endpoints. The user-generated data plane endpoints are serviced with REST.
pub struct RpcService {
    api: Arc<Mutex<ApiService>>,
    type_system: Arc<Mutex<TypeSystem>>,
}

impl RpcService {
    pub fn new(api: Arc<Mutex<ApiService>>, type_system: Arc<Mutex<TypeSystem>>) -> Self {
        RpcService { api, type_system }
    }
}

#[tonic::async_trait]
impl ChiselRpc for RpcService {
    /// Get Chisel server status.
    async fn get_status(
        &self,
        _request: Request<StatusRequest>,
    ) -> Result<Response<StatusResponse>, Status> {
        let response = chisel::StatusResponse {
            message: "OK".to_string(),
        };
        Ok(Response::new(response))
    }

    /// Define a type.
    async fn define_type(
        &self,
        request: Request<TypeDefinitionRequest>,
    ) -> Result<Response<TypeDefinitionResponse>, Status> {
        let mut type_system = self.type_system.lock().await;
        let name = request.into_inner().name;
        type_system.define_type(Type {
            name: name.to_owned(),
        });
        let path = format!("/{}", name.to_case(Case::Snake));
        info!("Registered endpoint: '{}'", path);
        self.api.lock().await.get(
            &path,
            Box::new(|| {
                // Let's return an empty array because we don't do storage yet.
                let result = json!([]);
                result.to_string()
            }),
        );
        let response = chisel::TypeDefinitionResponse { message: name };
        Ok(Response::new(response))
    }

    async fn export_types(
        &self,
        _request: tonic::Request<TypeExportRequest>,
    ) -> Result<tonic::Response<TypeExportResponse>, tonic::Status> {
        let type_system = self.type_system.lock().await;
        let mut type_defs = vec![];
        for ty in type_system.types.values() {
            let type_def = chisel::TypeDefinition {
                name: ty.name.to_string(),
                field_defs: vec![],
            };
            type_defs.push(type_def);
        }
        let response = chisel::TypeExportResponse { type_defs };
        Ok(Response::new(response))
    }
}

pub fn spawn(
    rpc: RpcService,
    addr: SocketAddr,
    shutdown: impl core::future::Future<Output = ()> + Send + 'static,
) -> tokio::task::JoinHandle<Result<(), tonic::transport::Error>> {
    tokio::spawn(async move {
        let ret = Server::builder()
            .add_service(ChiselRpcServer::new(rpc))
            .serve_with_shutdown(addr, shutdown)
            .await;
        info!("Tonic shutdown");
        ret
    })
}