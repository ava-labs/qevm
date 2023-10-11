use actix::prelude::*;

#[derive(Message, Debug)]
#[rtype(result = "Option<crate::common::WrappedHash>")]
pub struct AddTransaction(pub crate::common::Bytes);

#[derive(Message, Debug)]
#[rtype(result = "crate::common::WrappedWei")]
pub struct GasPrice;

#[derive(Message, Debug)]
#[rtype(result = "()")]
pub struct GenBlocks;

pub trait MempoolService:
    Actor<Context = Context<Self>>
    + Handler<AddTransaction>
    + Handler<GenBlocks>
    + Handler<GasPrice>
{
}
