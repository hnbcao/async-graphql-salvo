mod books;

use salvo::conn::TcpListener;
use salvo::{Listener, Server};

#[tokio::main]
async fn main() {
    // Initialize logging subsystem
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::TRACE)
        .init();

    // Bind server to port 5800
    let acceptor = TcpListener::new("0.0.0.0:5800").bind().await;

    // Create graphql router
    let router = graphql::router();
    // let router = user::router(router);

    // Print router structure for debugging
    tracing::info!("{:?}", router);

    // Start serving requests
    Server::new(acceptor).serve(router).await;
}

mod graphql {
    use crate::books::{MutationRoot, QueryRoot, Storage, SubscriptionRoot};
    use async_graphql::http::GraphiQLSource;
    use async_graphql::Schema;
    use async_graphql_salvo::subscription::GraphQLSubscription;
    use async_graphql_salvo::GraphQL;
    use salvo::prelude::Text;
    use salvo::{handler, Request, Response, Router};

    #[handler]
    async fn graphiql(_req: &mut Request, res: &mut Response) {
        let document = GraphiQLSource::build()
            .endpoint("/")
            .subscription_endpoint("/ws")
            .finish();
        res.render(Text::Html(document));
    }

    pub(crate) fn router() -> Router {
        let schema = Schema::build(QueryRoot, MutationRoot, SubscriptionRoot)
            .data(Storage::default())
            .finish();
        Router::new()
            .get(graphiql)
            .post(GraphQL::new(schema.clone()))
            .push(Router::with_path("ws").goal(GraphQLSubscription::new(schema)))
    }
}
