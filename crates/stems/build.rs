//! Generates the ONNX protobuf types (`onnx.rs`) from the committed `model/onnx.proto3`
//! (ONNX v1.23.2) with `protox`, so no `protoc` binary is needed.

fn main() {
    let proto = "model/onnx.proto3";
    println!("cargo::rerun-if-changed={proto}");
    let descriptors = protox::compile([proto], ["model"]).expect("compiling model/onnx.proto3");
    prost_build::Config::new()
        .compile_fds(descriptors)
        .expect("generating the ONNX protobuf types");
}
