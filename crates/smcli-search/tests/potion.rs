use smcli_search::Potion;

/// reference.json: model2vec (Python) embeddings of the shipped model, the same file the Swift
/// `Match model2vec reference embeddings` test passes against.
#[test]
fn match_model2vec_reference_embeddings() {
    let reference: serde_json::Value = serde_json::from_str(include_str!("reference.json")).unwrap();
    let model = Potion::bundled().unwrap();
    for (key, entry) in reference["texts"].as_object().unwrap() {
        let text = entry["text"].as_str().unwrap().to_string();
        let got = model.embed(&[text]).remove(0);
        let want: Vec<f32> = reference["embeddings"][key].as_array().unwrap().iter().map(|v| v.as_f64().unwrap() as f32).collect();
        assert_eq!(got.len(), 256);
        let cosine: f32 = got.iter().zip(&want).map(|(a, b)| a * b).sum::<f32>() / want.iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!(cosine > 0.9999, "{key}: {cosine}");
    }
    assert_eq!(model.embed(&["".to_string()])[0], vec![1.0 / 16.0; 256]);
}
