fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Only compile protos when the grpc feature is enabled
    if std::env::var("CARGO_FEATURE_GRPC").is_ok() {
        tonic_prost_build::configure()
            .build_server(true)
            .build_client(true)
            .compile_protos(&["proto/this_grpc.proto"], &["proto"])?;
    }
    Ok(())
}
