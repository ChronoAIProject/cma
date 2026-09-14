fn main() {
    println!(
        "{}",
        layer1_public::rest::openapi::document()
            .to_pretty_json()
            .expect("The public OpenAPI document is serializable")
    );
}
