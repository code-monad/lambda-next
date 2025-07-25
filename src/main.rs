use lambda_next::server::App;
use anyhow::Result;

#[tokio::main]
async fn main() -> Result<()> {
    let app = App::new().await?;
    app.run().await
}
