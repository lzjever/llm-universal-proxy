use serde_json::Value;

use super::models::{NormalizedResponseLogprobCandidate, NormalizedResponseTokenLogprob};
use super::openai_responses::{
    responses_hosted_output_item_type, responses_portable_output_item_type,
};

pub(super) fn responses_nonportable_output_item_message(
    item: &Value,
    target_label: &str,
    allow_reasoning_encrypted_content: bool,
) -> Option<String> {
    let item_type = item.get("type").and_then(Value::as_str)?;
    match item_type {
        "reasoning"
            if item.get("encrypted_content").is_some() && !allow_reasoning_encrypted_content =>
        {
            Some(format!(
            "OpenAI Responses reasoning output item field `encrypted_content` cannot be faithfully translated to {target_label}"
        ))
        }
        "function_call_output" | "custom_tool_call_output" => Some(format!(
            "OpenAI Responses output item `{item_type}` cannot be faithfully translated to {target_label}"
        )),
        "compaction" => Some(format!(
            "OpenAI Responses output item `compaction` cannot be faithfully translated to {target_label}"
        )),
        _ if responses_portable_output_item_type(item_type) => None,
        _ if responses_hosted_output_item_type(item_type) => Some(format!(
            "OpenAI Responses output item `{item_type}` cannot be faithfully translated to {target_label}"
        )),
        _ => Some(format!(
            "OpenAI Responses output item `{item_type}` is outside the portable cross-protocol subset and cannot be faithfully translated to {target_label}"
        )),
    }
}

pub(super) fn responses_output_text_logprobs(item: &Value) -> Result<Option<Vec<Value>>, String> {
    let Some(parts) = item.get("content").and_then(Value::as_array) else {
        return Ok(None);
    };

    let mut saw_logprobs = false;
    let mut content_logprobs = Vec::new();
    for part in parts {
        if part.get("type").and_then(Value::as_str) != Some("output_text") {
            continue;
        }
        match part.get("logprobs") {
            Some(Value::Array(logprobs)) => {
                saw_logprobs = true;
                content_logprobs.extend(logprobs.iter().cloned());
            }
            Some(Value::Null) | None => {}
            Some(_) => {
                return Err(
                    "OpenAI Responses message.output_text.logprobs must be an array for response translation."
                        .to_string(),
                )
            }
        }
    }

    Ok(saw_logprobs.then_some(content_logprobs))
}

pub(super) fn normalized_response_logprob_candidate_from_value(
    value: &Value,
    field_name: &str,
    target_label: &str,
) -> Result<NormalizedResponseLogprobCandidate, String> {
    let Some(obj) = value.as_object() else {
        return Err(format!(
            "OpenAI-family response field `{field_name}` must contain objects when translating to {target_label}"
        ));
    };
    let token = obj
        .get("token")
        .and_then(Value::as_str)
        .filter(|token| !token.is_empty())
        .ok_or_else(|| {
            format!(
                "OpenAI-family response field `{field_name}` must contain non-empty string `token` values when translating to {target_label}"
            )
        })?
        .to_string();
    let logprob = obj
        .get("logprob")
        .and_then(Value::as_f64)
        .filter(|logprob| logprob.is_finite())
        .ok_or_else(|| {
            format!(
                "OpenAI-family response field `{field_name}` must contain finite numeric `logprob` values when translating to {target_label}"
            )
        })?;
    Ok(NormalizedResponseLogprobCandidate {
        raw: value.clone(),
        token,
        logprob,
    })
}

pub(super) fn normalized_response_token_logprob_from_value(
    value: &Value,
    target_label: &str,
) -> Result<NormalizedResponseTokenLogprob, String> {
    let candidate = normalized_response_logprob_candidate_from_value(
        value,
        "choice.logprobs.content",
        target_label,
    )?;
    let top_logprobs = match value.get("top_logprobs") {
        Some(Value::Array(items)) => items
            .iter()
            .map(|item| {
                normalized_response_logprob_candidate_from_value(
                    item,
                    "choice.logprobs.content[].top_logprobs",
                    target_label,
                )
            })
            .collect::<Result<Vec<_>, _>>()?,
        Some(Value::Null) | None => Vec::new(),
        Some(_) => {
            return Err(format!(
                "OpenAI Chat response field `choice.logprobs.content[].top_logprobs` must be an array when translating to {target_label}"
            ))
        }
    };
    Ok(NormalizedResponseTokenLogprob {
        raw: value.clone(),
        token: candidate.token,
        logprob: candidate.logprob,
        top_logprobs,
    })
}

pub(super) fn normalized_response_logprobs_from_openai_choice(
    choice: &Value,
    target_label: &str,
) -> Result<Option<Vec<NormalizedResponseTokenLogprob>>, String> {
    let Some(logprobs) = choice.get("logprobs").filter(|value| !value.is_null()) else {
        return Ok(None);
    };
    let Some(logprobs) = logprobs.as_object() else {
        return Err(format!(
            "OpenAI Chat response field `choice.logprobs` must be an object when translating to {target_label}"
        ));
    };
    if logprobs
        .get("refusal")
        .and_then(Value::as_array)
        .is_some_and(|refusal| !refusal.is_empty())
    {
        return Err(format!(
            "OpenAI Chat response field `choice.logprobs.refusal` cannot be faithfully translated to {target_label}"
        ));
    }
    match logprobs.get("content") {
        Some(Value::Array(content)) => content
            .iter()
            .map(|item| normalized_response_token_logprob_from_value(item, target_label))
            .collect::<Result<Vec<_>, _>>()
            .map(Some),
        Some(Value::Null) | None => Ok(None),
        Some(_) => Err(format!(
            "OpenAI Chat response field `choice.logprobs.content` must be an array when translating to {target_label}"
        )),
    }
}

pub(super) fn normalized_response_logprob_candidate_from_gemini_value(
    value: &Value,
    field_name: &str,
    target_label: &str,
) -> Result<NormalizedResponseLogprobCandidate, String> {
    let Some(obj) = value.as_object() else {
        return Err(format!(
            "Gemini response field `{field_name}` must contain objects when translating to {target_label}"
        ));
    };
    let token = obj
        .get("token")
        .and_then(Value::as_str)
        .filter(|token| !token.is_empty())
        .ok_or_else(|| {
            format!(
                "Gemini response field `{field_name}` must contain non-empty string `token` values when translating to {target_label}"
            )
        })?
        .to_string();
    let logprob = obj
        .get("logProbability")
        .and_then(Value::as_f64)
        .filter(|logprob| logprob.is_finite())
        .ok_or_else(|| {
            format!(
                "Gemini response field `{field_name}` must contain finite numeric `logProbability` values when translating to {target_label}"
            )
        })?;
    Ok(NormalizedResponseLogprobCandidate {
        raw: serde_json::json!({
            "token": token,
            "logprob": logprob
        }),
        token,
        logprob,
    })
}

pub(super) fn normalized_response_logprobs_from_gemini_candidate(
    candidate: &Value,
    target_label: &str,
) -> Result<Option<Vec<NormalizedResponseTokenLogprob>>, String> {
    let avg_logprobs = match candidate.get("avgLogprobs").filter(|value| !value.is_null()) {
        Some(value) => Some(
            value
                .as_f64()
                .filter(|logprob| logprob.is_finite())
                .ok_or_else(|| {
                    format!(
                        "Gemini response field `avgLogprobs` must be a finite number when translating to {target_label}"
                    )
                })?,
        ),
        None => None,
    };
    let Some(logprobs_result) = candidate
        .get("logprobsResult")
        .filter(|value| !value.is_null())
    else {
        if avg_logprobs.is_some() {
            return Err(format!(
                "Gemini response field `avgLogprobs` cannot be faithfully translated to {target_label} without token-level `logprobsResult`"
            ));
        }
        return Ok(None);
    };
    let Some(logprobs_result) = logprobs_result.as_object() else {
        return Err(format!(
            "Gemini response field `logprobsResult` must be an object when translating to {target_label}"
        ));
    };
    let chosen_candidates = logprobs_result
        .get("chosenCandidates")
        .and_then(Value::as_array)
        .ok_or_else(|| {
            format!(
                "Gemini response field `logprobsResult.chosenCandidates` must be an array when translating to {target_label}"
            )
        })?;
    let top_candidates = match logprobs_result.get("topCandidates") {
        Some(Value::Array(items)) => Some(items),
        Some(Value::Null) | None => None,
        Some(_) => {
            return Err(format!(
                "Gemini response field `logprobsResult.topCandidates` must be an array when translating to {target_label}"
            ))
        }
    };
    if let Some(top_candidates) = top_candidates {
        if top_candidates.len() != chosen_candidates.len() {
            return Err(format!(
                "Gemini response fields `logprobsResult.chosenCandidates` and `logprobsResult.topCandidates` must have the same length when translating to {target_label}"
            ));
        }
    }

    chosen_candidates
        .iter()
        .enumerate()
        .map(|(idx, chosen_candidate)| {
            let chosen = normalized_response_logprob_candidate_from_gemini_value(
                chosen_candidate,
                "logprobsResult.chosenCandidates",
                target_label,
            )?;
            let top_logprobs = if let Some(top_candidates) = top_candidates {
                let top_candidates_for_step = top_candidates
                    .get(idx)
                    .and_then(|step| step.get("candidates"))
                    .and_then(Value::as_array)
                    .ok_or_else(|| {
                        format!(
                            "Gemini response field `logprobsResult.topCandidates[].candidates` must be an array when translating to {target_label}"
                        )
                    })?;
                top_candidates_for_step
                    .iter()
                    .map(|top_candidate| {
                        normalized_response_logprob_candidate_from_gemini_value(
                            top_candidate,
                            "logprobsResult.topCandidates[].candidates",
                            target_label,
                        )
                    })
                    .collect::<Result<Vec<_>, _>>()?
            } else {
                Vec::new()
            };
            Ok(NormalizedResponseTokenLogprob {
                raw: serde_json::json!({
                    "token": chosen.token,
                    "logprob": chosen.logprob,
                    "top_logprobs": top_logprobs
                        .iter()
                        .map(|candidate| candidate.raw.clone())
                        .collect::<Vec<_>>()
                }),
                token: chosen.token,
                logprob: chosen.logprob,
                top_logprobs,
            })
        })
        .collect::<Result<Vec<_>, String>>()
        .map(Some)
}

pub(super) fn normalized_response_logprobs_to_openai_values(
    content_logprobs: &[NormalizedResponseTokenLogprob],
) -> Vec<Value> {
    content_logprobs
        .iter()
        .map(|item| item.raw.clone())
        .collect::<Vec<_>>()
}

pub(super) fn attach_openai_choice_logprobs_to_responses_content(
    content: &mut [Value],
    content_logprobs: &[NormalizedResponseTokenLogprob],
) -> Result<(), String> {
    let output_text_indexes = content
        .iter()
        .enumerate()
        .filter_map(|(idx, part)| {
            (part.get("type").and_then(Value::as_str) == Some("output_text")).then_some(idx)
        })
        .collect::<Vec<_>>();
    let [output_text_index] = output_text_indexes.as_slice() else {
        return Err(
            "OpenAI Chat response logprobs can only be translated to Responses when assistant output maps to a single `output_text` item."
                .to_string(),
        );
    };
    content[*output_text_index]["logprobs"] = Value::Array(
        normalized_response_logprobs_to_openai_values(content_logprobs),
    );
    Ok(())
}

pub(super) fn normalized_response_logprob_candidate_to_gemini(
    candidate: &NormalizedResponseLogprobCandidate,
) -> Value {
    serde_json::json!({
        "token": candidate.token,
        "logProbability": candidate.logprob
    })
}

pub(super) fn normalized_response_logprobs_to_gemini_fields(
    content_logprobs: &[NormalizedResponseTokenLogprob],
) -> (Option<Value>, Value) {
    let log_probability_sum = content_logprobs
        .iter()
        .map(|item| item.logprob)
        .sum::<f64>();
    let avg_logprobs = (!content_logprobs.is_empty())
        .then(|| serde_json::json!(log_probability_sum / content_logprobs.len() as f64));
    let logprobs_result = serde_json::json!({
        "chosenCandidates": content_logprobs
            .iter()
            .map(|item| serde_json::json!({
                "token": item.token,
                "logProbability": item.logprob
            }))
            .collect::<Vec<_>>(),
        "topCandidates": content_logprobs
            .iter()
            .map(|item| serde_json::json!({
                "candidates": item
                    .top_logprobs
                    .iter()
                    .map(normalized_response_logprob_candidate_to_gemini)
                    .collect::<Vec<_>>()
            }))
            .collect::<Vec<_>>(),
        "logProbabilitySum": log_probability_sum
    });
    (avg_logprobs, logprobs_result)
}
