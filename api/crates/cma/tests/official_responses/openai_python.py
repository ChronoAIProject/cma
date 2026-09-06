import json
import os
from urllib.parse import urlparse

import openai

base_url = os.environ["CMA_SDK_BASE_URL"]
endpoint = urlparse(base_url)
assert endpoint.scheme == "http"
assert endpoint.hostname in {"127.0.0.1", "localhost", "::1"}
client = openai.OpenAI(
    base_url=base_url, api_key="local-sdk-conformance", max_retries=0, timeout=30
)

conversation = client.conversations.create()
assert conversation.id.startswith("agt_")
assert conversation.object == "conversation"
for field in ["sandbox", "workspace", "agent_profile"]:
    assert isinstance(conversation.metadata[field], str)
assert client.conversations.retrieve(conversation.id) == conversation

frames = list(client.responses.create(
    conversation=conversation.id,
    model="deployment-default",
    input=[{"role": "user", "content": [{"type": "input_text", "text": "sdk-raw-with-tools"}]}],
    metadata={"proof": "python-raw"},
    stream=True,
))
assert frames[0].type == "response.created"
assert frames[-1].type == "response.completed"
assert [event.sequence_number for event in frames] == list(range(len(frames)))
assert any(event.type == "response.reasoning_summary_text.delta" for event in frames)
assert any(event.type == "response.cma_chat.tool_activity" for event in frames)
first = frames[-1].response
first_read = client.responses.retrieve(first.id)
assert first_read.output == first.output
assert first_read.metadata == {"proof": "python-raw"}
assert first_read.output_text == "Hello from CMA"
assert all(item.type == "message" for item in first_read.output)

items = list(client.conversations.items.list(conversation.id, limit=1))
assert any(item.id == first.output[0].id for item in items)
assert any(item.type == "cma_chat.tool_activity" for item in items)
assert all(item.type != "reasoning" for item in items)

with client.responses.stream(
    conversation=conversation.id,
    model="deployment-default",
    input="sdk-helper-text",
    previous_response_id=first.id,
) as stream:
    helper_frames = list(stream)
    final = stream.get_final_response()
assert final.status == "completed"
assert final.previous_response_id == first.id
assert final.output_text == "Hello from CMA"
assert any(event.type == "response.reasoning_summary_text.delta" for event in helper_frames)
assert any(event.type == "response.output_item.added" and event.item.type == "reasoning" for event in helper_frames)
assert all(item.type == "message" for item in final.output)
assert [(item.id, [part.text for part in item.content]) for item in client.responses.retrieve(final.id).output] == [
    (item.id, [part.text for part in item.content]) for item in final.output
]
try:
    client.responses.create(conversation=conversation.id, input="Must not fork", stream=True)
except openai.APIStatusError as error:
    assert error.status_code == 409
    assert error.body["code"] == "previous_response_id_conflict"
else:
    raise AssertionError("The linear predecessor conflict was not enforced")

print(json.dumps({"sdk": "openai-python", "version": openai.__version__,
                  "conversation": conversation.id, "raw_frames": len(frames),
                  "helper_frames": len(helper_frames), "items": len(items), "status": "passed"}))
