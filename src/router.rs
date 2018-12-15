use ::actix::dev::*;
use anymap::AnyMap;
use failure::{Error, Fail};
use futures::{future, Future};
use log::*;
use serde::{de::DeserializeOwned, Serialize};
use serde_derive::{Deserialize, Serialize};

use std::marker::PhantomData;
use std::ops::Deref;

use crate::get_type;

/// A lookup from `Message` types to addresses to request handlers.
/// This is encapsulated by an `AnyMap`, but the methods `insert_handler`, 
/// and `insert_handler_fut` ensure that only `Route<M: SoarMessage>`s are
/// actually added (or retrieved).
pub struct Router {
    pub routes: AnyMap,
}

impl std::default::Default for Router {
    fn default() -> Self {
        Self::new()
    }
}

impl actix::Supervised for Router {}

impl ArbiterService for Router {}

impl Actor for Router {
    type Context = actix::Context<Self>;

    fn started(&mut self, _ctx: &mut Self::Context) {
        trace!("Started router on arbiter: {}", Arbiter::name());
    }

    fn stopped(&mut self, _ctx: &mut Self::Context) {
        trace!("Stopped router on arbiter: {}", Arbiter::name());
    }
}

/// An unfortunate type signature...
/// The inner type is `Box<Any>` which is intentionally hiding the actual
/// type signature `Recipient<M>`, since we want to erase the type.
/// (This type is recovered through the map's key).
/// Next, `Shared` requires a `Future: Sized` implementation, which we
/// also need to box apparently
/// And finally, the `Shared` itself needs to be boxed, 
type PendingRoute<M> = future::Shared<Box<Future<Item=Recipient<M>, Error=()>>>;

/// A `Route` is either a straightforward `Recipient<M>`, which is stored in the
/// table as an `Any` and downcasted back to a `Recipient<M>`. (We make sure
/// this invariant stays true by limiting the places where we add to the table).
///
/// Or, the pending type contains a future which eventually resolves to the same
/// value. While in the pending states, requests to this route are handled by
/// cloning the future and combining with the request. TODO: In the case where
/// the startup takes a while, can this cause congestion issues?
pub enum Route<M: SoarMessage> {
    Pending(PendingRoute<M>),
    Done(Recipient<M>),
}

impl<M: SoarMessage> Clone for Route<M> {
    fn clone(&self) -> Self {
        match self {
            Route::Pending(r) => Route::Pending(r.clone()),
            Route::Done(r) => Route::Done(r.clone()),
        }
    }
}

impl Router {
    /// Create a new router.
    pub fn new() -> Self {
        Router {
            routes: AnyMap::new(),
        }
    }

    /// Add this address into the routing table.
    pub fn insert_handler<M: SoarMessage>(&mut self, handler: Recipient<M>) {
        self.routes.insert(
            Route::Done(handler)
        );
    }

    /// Delete a handler from the routing table.
    pub fn remove_handler<M>(&mut self)
        where M: SoarMessage
    {
        self.routes.remove::<M>();
    }


    /// Get the handler identified by the generic type parameter `M`.
    pub fn get_recipient<M>(&self) -> impl Future<Item=Recipient<M>, Error=()>
        where  M: SoarMessage,
    {
        trace!("Lookup request handler for {:?} on arbiter: {}", get_type!(M), Arbiter::name());
        future::result({
            self.routes.get().cloned().ok_or_else(|| {
                debug!("No route found for {:?}", get_type!(M));
            })
        }).and_then(|h| {
            match h {
                Route::Pending(fut) => {
                    trace!("Handler pending, composing with future");
                    future::Either::A(
                        fut.map(|recip| {
                            recip.deref().clone()
                        }).map_err(|_| ())
                    )
                },
                Route::Done(recip) => {
                    trace!("Route exists, converting handler");
                    future::Either::B(
                       future::ok(recip)
                    )
                }
            }
        })
        
    }
}

/// Instruct the service to add this service to the routing table.
pub(crate) struct AddRoute<M>(pub Recipient<M>)
    where M: SoarMessage;

impl<M> Message for AddRoute<M>
    where M: SoarMessage,
{
    type Result = ();
}

impl<M> Handler<AddRoute<M>> for Router
where M: SoarMessage,
{
    type Result = ();

    fn handle(&mut self, msg: AddRoute<M>, _ctxt: &mut Context<Self>) {
        self.insert_handler(msg.0);
    }
}

/// Instruct the service to schedule a new future which resolves to a new
/// handler.
pub(crate) struct AddRouteFuture<M, F>
    where M: SoarMessage,
          F: Future<Item=Recipient<M>, Error=()>
{
    pub fut: F,
    // pub _type: ::std::marker::PhantomData<M>,
}

impl<M, F> Message for AddRouteFuture<M, F>
    where M: SoarMessage,
          F: Future<Item=Recipient<M>, Error=()>
{
    type Result = ();
}

impl<M, F> Handler<AddRouteFuture<M, F>> for Router
where M: SoarMessage,
      F: 'static + Future<Item=Recipient<M>, Error=()>
{
    type Result = ();

    fn handle(&mut self, msg: AddRouteFuture<M, F>, ctxt: &mut Context<Self>) {
        let fut: Box<Future<Item=Recipient<M>, Error=()>> = Box::new(msg.fut);
        let shared = fut.shared();
        let fut = shared.clone().into_actor(&*self).map(|recip, router, _ctxt| {
            router.insert_handler(recip.deref().clone());
            recip
        });
        ctxt.spawn(fut.map(|_, _ ,_| ()).map_err(|_, _, _| ()));
        self.routes.insert(Route::Pending(shared));
    }
}

/// Instruct the service to add this service to the routing table.
pub(crate) struct RemoveRoute<M>(pub PhantomData<M>)
    where M: SoarMessage;

impl<M> Message for RemoveRoute<M>
    where M: SoarMessage,
{
    type Result = ();
}

impl<M> Handler<RemoveRoute<M>> for Router
where M: SoarMessage,
{
    type Result = ();

    fn handle(&mut self, _msg: RemoveRoute<M>, _ctxt: &mut Context<Self>) {
        self.remove_handler::<M>();
    }
}

#[derive(Default, Deserialize, Serialize, Fail, Debug)]
#[fail(display = "routing error found")]
/// `Router` fails when there is no known handler for a given message.
pub struct RouterError {}

impl<M> Handler<M> for Router
    where M: SoarMessage
{
    type Result = SoarResponse<M>;

    fn handle(&mut self, msg: M, _ctxt: &mut Context<Self>) -> Self::Result {
        let handler = self.get_recipient::<M>()
                          .map_err(|_| Error::from(RouterError::default()));
        SoarResponse(Box::new(handler.and_then(|h| h.send(msg).map_err(Error::from))))
    }
}


/// Helper type for messages (`actix::Message`) which can be processed by `soar`.
/// Requires the inputs/outputs to be de/serializable so that they can be sent over
/// a network.
pub trait SoarMessage:
    Message<Result=<Self as SoarMessage>::Response>
    + 'static + Send + DeserializeOwned + Serialize
{
    type Response: 'static + Send + DeserializeOwned + Serialize;
}

/// Wrapper type for a response to a `SoarMessage`. 
/// Since this implements `actix::MessageResponse`, we can use it as the return type from
/// a `Handler` implementation. 
pub struct SoarResponse<M: SoarMessage>(pub Box<Future<Item=M::Response, Error=Error>>);

impl<F, M> From<F> for SoarResponse<M>
    where
        M: SoarMessage, 
        F: 'static + Future<Item=M::Response, Error=Error>,
{
    fn from(other: F) -> Self {
        SoarResponse(Box::new(other))
    }
}

impl<A, M> MessageResponse<A, M> for SoarResponse<M>
    where 
        A: Actor<Context=Context<A>>,
        M: SoarMessage,
{
    fn handle<R: ResponseChannel<M>>(self, _ctxt: &mut Context<A>, tx: Option<R>) {
        Arbiter::spawn(self.0.and_then(move |res| {
            if let Some(tx) = tx {
                tx.send(res)
            }
            Ok(())
        }).map_err(|_| ()));
    }
}

/// Register this recipient (usually `Addr<Actor>`) as handling a given message type
pub fn add_route<M, R>(handler: R)
    where M: SoarMessage,
          R: 'static + Into<Recipient<M>>
{
    trace!("Add handler {:?} for {:?}", get_type!(R), get_type!(M));
    send_spawn(AddRoute(handler.into()))
}

/// Set the completion of the future to handle messages of type `M`.
/// Any messages for this address in the meantime will be chained
/// onto the future.
// pub fn add_route_fut<M, R, F>(fut: F)
//     where M: SoarMessage,
//           R: Into<Recipient<M>>,
//           F: 'static + Future<Item=R, Error=()> + Send,
// {
//     send_spawn(AddRouteFuture { fut: fut.map(|r| r.into()) });
// }

/// Delete the route
// pub fn del_route<M>()
//     where M: SoarMessage
// {
//     send_spawn(RemoveRoute(std::marker::PhantomData::<M>));
// }

fn send_spawn<M>(msg: M)
    where Router: Handler<M>,
          M: 'static + Message + Send,
          M::Result: Send,
{
    Arbiter::spawn(
        send(msg).map(|_| ()).map_err(|_| ())
    );
}

/// Send a message to this service. Looks up the route registered
/// with `add_route` et al. and sends the message.
pub fn send<M>(msg: M) -> impl Future<Item=M::Result, Error=Error>
    where Router: Handler<M>,
          M: 'static + Message + Send,
          M::Result: Send,
{
    Arbiter::registry().get::<Router>().send(msg)
        .map_err(Error::from)
}