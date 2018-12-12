use ::actix::dev::*;
use failure::Error;
use futures::{future, Future};
use log::*;
use serde_derive::{Deserialize, Serialize};

use std::sync::Once;

static START: Once = Once::new();

use crate::*;

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct TestMessage(pub u8);
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct TestResponse(pub u8);

impl Message for TestMessage {
	type Result = TestResponse;
}

impl SoarMessage for TestMessage {
	type Response = TestResponse;
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct TestMessageEmpty;

impl Message for TestMessageEmpty {
	type Result = ();
}

impl SoarMessage for TestMessageEmpty {
	type Response = ();
}

#[derive(Default)]
pub struct TestHandler;

impl Actor for TestHandler {
	type Context = Context<Self>;
}

impl Handler<TestMessage> for TestHandler {
	type Result = MessageResult<TestMessage>;

	fn handle(&mut self, msg: TestMessage, _ctxt: &mut Context<Self>) -> Self::Result {
		trace!("Handling TestMessage from TestHandler");
		MessageResult(TestResponse(msg.0))
	}
}

impl Handler<TestMessageEmpty> for TestHandler {
	type Result = ();

	fn handle(&mut self, msg: TestMessageEmpty, _ctxt: &mut Context<Self>) {
		trace!("Handling TestMessageEmpty from TestHandler");
	}
}

#[derive(Default)]
pub struct TestIntoHandler(pub u8);

impl Actor for TestIntoHandler {
	type Context = Context<Self>;
}

impl Handler<TestMessageEmpty> for TestIntoHandler {
	type Result = SoarResponse<TestMessageEmpty>;

	fn handle(&mut self, _msg: TestMessageEmpty, _ctxt: &mut Context<Self>) -> Self::Result {
		trace!("Handling TestMessageEmpty from TestIntoHandler");
		SoarResponse(Box::new(service::send(TestMessage(42)).map(|_| ())))
	}
}

pub fn init_logger() {
    START.call_once(|| {
    	if std::env::var("TEST_LOG").is_ok() {
		    ::std::env::set_var("RUST_LOG", format!("actix_web={1},actix={0},soar={1}", "trace", "trace"));
    	}
	    env_logger::init();
    });
}
