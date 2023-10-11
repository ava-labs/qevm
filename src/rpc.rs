use actix::prelude::*;
use jsonrpc_v2::{Data, Error, Params, Server};

use crate::chain::{BlockNumber, ChainStateService};
use crate::common::{
    Addr, Bytes, Hash, Wei, WrappedGas, WrappedU256, WrappedU64,
};
use crate::mempool::MempoolService;

const INTERNAL_ERROR: Error = Error::Provided {
    code: -1,
    message: "internal error",
};

const INVALID_REQUEST: Error = Error::Provided {
    code: -1,
    message: "invalid request",
};

async fn send_raw_tx<C: ChainStateService, M: MempoolService>(
    srv: Data<RPCState<C, M>>, Params(raw): Params<(Bytes,)>,
) -> Result<Hash, Error> {
    srv.mempool
        .send(crate::mempool::AddTransaction(raw.0))
        .await
        .map_err(|_| INTERNAL_ERROR)
        .and_then(|h| h.ok_or(INVALID_REQUEST))
        .map(|h| h.0)
}

async fn get_tx_count<C: ChainStateService, M: MempoolService>(
    srv: Data<RPCState<C, M>>, Params(params): Params<(Addr, BlockNumber)>,
) -> Result<WrappedU64, Error> {
    srv.chain_state
        .send(crate::chain::GetNonce(params.0, params.1))
        .await
        .map_err(|_| INTERNAL_ERROR)
        .and_then(|h| h.ok_or(INVALID_REQUEST))
}

async fn get_balance<C: ChainStateService, M: MempoolService>(
    srv: Data<RPCState<C, M>>, Params(params): Params<(Addr, BlockNumber)>,
) -> Result<Wei, Error> {
    srv.chain_state
        .send(crate::chain::GetBalance(params.0, params.1))
        .await
        .map_err(|_| INTERNAL_ERROR)
        .and_then(|h| h.ok_or(INVALID_REQUEST))
        .map(|h| h.0)
}

async fn block_number<C: ChainStateService, M: MempoolService>(
    srv: Data<RPCState<C, M>>,
) -> Result<WrappedU256, Error> {
    srv.chain_state
        .send(crate::chain::GetTailBlock)
        .await
        .map_err(|_| INTERNAL_ERROR)
        .map(|b| b.0)
}

async fn call<C: ChainStateService, M: MempoolService>(
    srv: Data<RPCState<C, M>>,
    Params(params): Params<(crate::chain::TxArgs, BlockNumber)>,
) -> Result<Bytes, Error> {
    println!("call");
    if params.0.to.is_none() {
        // optional by eth_estimateGas, required by eth_call
        return Err(INTERNAL_ERROR)
    }
    use crate::core::TxExecResult::*;
    srv.chain_state
        .send(crate::chain::Call(params.0, params.1))
        .await
        .map_err(|_| INTERNAL_ERROR)
        .and_then(|h| h.ok_or(INVALID_REQUEST))
        .map(|h| match h.0 {
            Succeeded(d, _, _) => d,
            Reverted(d, _, _) => d,
        })
}

async fn get_tx_receipt<C: ChainStateService, M: MempoolService>(
    srv: Data<RPCState<C, M>>, Params(params): Params<(Hash,)>,
) -> Result<Option<crate::chain::TxReceipt>, Error> {
    srv.chain_state
        .send(crate::chain::GetTxReceipt(params.0))
        .await
        .map_err(|_| INTERNAL_ERROR)
}

async fn get_tx_by_hash<C: ChainStateService, M: MempoolService>(
    srv: Data<RPCState<C, M>>, Params(params): Params<(Hash,)>,
) -> Result<Option<crate::chain::TxResponse>, Error> {
    srv.chain_state
        .send(crate::chain::GetTxByHash(params.0))
        .await
        .map_err(|_| INTERNAL_ERROR)
}

async fn net_version<C: ChainStateService, M: MempoolService>(
    srv: Data<RPCState<C, M>>,
) -> Result<String, Error> {
    srv.chain_state
        .send(crate::chain::GetNetworkId)
        .await
        .map_err(|_| INTERNAL_ERROR)
}

async fn chain_id<C: ChainStateService, M: MempoolService>(
    srv: Data<RPCState<C, M>>,
) -> Result<WrappedU256, Error> {
    srv.chain_state
        .send(crate::chain::GetChainId)
        .await
        .map_err(|_| INTERNAL_ERROR)
}

async fn get_block_by_hash<C: ChainStateService, M: MempoolService>(
    srv: Data<RPCState<C, M>>, Params(params): Params<(Hash, bool)>,
) -> Result<crate::chain::BlockInfo, Error> {
    srv.chain_state
        .send(crate::chain::GetBlockByHash(params.0, params.1))
        .await
        .map_err(|_| INTERNAL_ERROR)
        .and_then(|h| h.ok_or(INVALID_REQUEST))
}

async fn get_block_by_number<C: ChainStateService, M: MempoolService>(
    srv: Data<RPCState<C, M>>, Params(params): Params<(BlockNumber, bool)>,
) -> Result<crate::chain::BlockInfo, Error> {
    srv.chain_state
        .send(crate::chain::GetBlockByNumber(params.0, params.1))
        .await
        .map_err(|_| INTERNAL_ERROR)
        .and_then(|h| h.ok_or(INVALID_REQUEST))
}

async fn get_storage_at<C: ChainStateService, M: MempoolService>(
    srv: Data<RPCState<C, M>>,
    Params(params): Params<(Addr, WrappedU256, BlockNumber)>,
) -> Result<crate::common::Bytes32, Error> {
    srv.chain_state
        .send(crate::chain::GetState(params.0, params.1, params.2))
        .await
        .map_err(|_| INTERNAL_ERROR)
        .and_then(|h| h.ok_or(INVALID_REQUEST))
        .map(|h| (&h.0).into())
}

async fn get_code<C: ChainStateService, M: MempoolService>(
    srv: Data<RPCState<C, M>>, Params(params): Params<(Addr, BlockNumber)>,
) -> Result<Bytes, Error> {
    srv.chain_state
        .send(crate::chain::GetCode(params.0, params.1))
        .await
        .map_err(|_| INTERNAL_ERROR)
        .and_then(|h| h.ok_or(INVALID_REQUEST))
}

async fn estimate_gas<C: ChainStateService, M: MempoolService>(
    srv: Data<RPCState<C, M>>,
    Params(params): Params<Vec<Box<serde_json::value::RawValue>>>,
) -> Result<WrappedGas, Error> {
    if params.len() > 2 {
        return Err(INVALID_REQUEST)
    }
    let tx_args: crate::chain::TxArgs =
        serde_json::from_str(params[0].get()).map_err(|_| INVALID_REQUEST)?;
    let block_number: BlockNumber = if params.len() < 2 {
        BlockNumber::Latest
    } else {
        serde_json::from_str(params[1].get()).map_err(|_| INVALID_REQUEST)?
    };
    srv.chain_state
        .send(crate::chain::EstimateGas(tx_args, block_number))
        .await
        .map_err(|_| INTERNAL_ERROR)
        .and_then(|h| h.ok_or(INVALID_REQUEST))
}

async fn gas_price<C: ChainStateService, M: MempoolService>(
    srv: Data<RPCState<C, M>>,
) -> Result<Wei, Error> {
    srv.mempool
        .send(crate::mempool::GasPrice)
        .await
        .map_err(|_| INTERNAL_ERROR)
        .map(|h| h.0)
}

async fn net_listening<C: ChainStateService, M: MempoolService>(
    _srv: Data<RPCState<C, M>>,
) -> Result<bool, Error> {
    Ok(true)
}

struct RPCState<C: ChainStateService, M: MempoolService> {
    chain_state: actix::Addr<C>,
    mempool: actix::Addr<M>,
}

impl<C: ChainStateService, M: MempoolService> RPCState<C, M> {
    fn new(chain_state: actix::Addr<C>, mempool: actix::Addr<M>) -> Self {
        Self {
            chain_state,
            mempool,
        }
    }
}

pub struct RPCServer<C: ChainStateService, M: MempoolService> {
    bind_addr: String,
    chain_state: actix::Addr<C>,
    mempool: actix::Addr<M>,
}

impl<C: ChainStateService, M: MempoolService> RPCServer<C, M> {
    pub fn new(
        bind_addr: &str, chain_state: &actix::Addr<C>, mempool: &actix::Addr<M>,
    ) -> Self {
        let bind_addr = bind_addr.into();
        let chain_state = chain_state.clone();
        let mempool = mempool.clone();
        Self {
            bind_addr,
            chain_state,
            mempool,
        }
    }
}

impl<C: ChainStateService, M: MempoolService> Actor for RPCServer<C, M> {
    type Context = Context<Self>;
    fn started(&mut self, ctx: &mut Self::Context) {
        println!("RPCServer started");
        let rpc = Server::new()
            .with_data(Data::new(RPCState::new(
                self.chain_state.clone(),
                self.mempool.clone(),
            )))
            .with_method("eth_sendRawTransaction", send_raw_tx::<C, M>)
            .with_method("eth_getBalance", get_balance::<C, M>)
            .with_method("eth_blockNumber", block_number::<C, M>)
            .with_method("eth_call", call::<C, M>)
            .with_method("eth_getTransactionCount", get_tx_count::<C, M>)
            .with_method("eth_getTransactionReceipt", get_tx_receipt::<C, M>)
            .with_method("eth_getTransactionByHash", get_tx_by_hash::<C, M>)
            .with_method("eth_getBlockByNumber", get_block_by_number::<C, M>)
            .with_method("eth_getBlockByHash", get_block_by_hash::<C, M>)
            .with_method("eth_getStorageAt", get_storage_at::<C, M>)
            .with_method("eth_getCode", get_code::<C, M>)
            .with_method("eth_chainId", chain_id::<C, M>)
            .with_method("eth_gasPrice", gas_price::<C, M>)
            .with_method("net_version", net_version::<C, M>)
            .with_method("eth_estimateGas", estimate_gas::<C, M>)
            .with_method("net_listening", net_listening::<C, M>)
            .finish();
        match actix_web::HttpServer::new(move || {
            let rpc = rpc.clone();
            let cors = actix_cors::Cors::permissive();
            actix_web::App::new().wrap(cors).service(
                actix_web::web::service("/api")
                    .guard(actix_web::guard::Post())
                    .finish(rpc.into_web_service()),
            )
        })
        .bind(&self.bind_addr)
        {
            Err(e) => println!("error: {}", e),
            Ok(r) => {
                let fut = async move {
                    if let Err(e) = r.run().await {
                        println!("server returns: {}", e);
                    }
                }
                .into_actor(self);
                ctx.wait(fut);
            }
        }
    }
    fn stopped(&mut self, _ctx: &mut Self::Context) {
        println!("RPCServer stopped");
    }
}
