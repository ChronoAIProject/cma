import json
import os
from urllib.parse import urlparse
from urllib.request import Request, urlopen
import openai

assert openai.__version__ == "3.8.0"
legacy_url = os.environ["CMA_SDK_BASE_URL"].rstrip("/")
endpoint = urlparse(legacy_url)
assert endpoint.scheme == "http" and endpoint.hostname in {"127.0.0.1", "localhost", "::1"}
assert legacy_url.endswith("/api/v1")
options = {"api_key": "local-sdk-conformance", "max_retries": 0, "timeout": 30}
legacy = openai.OpenAI(base_url=legacy_url, **options)
conversation = legacy.conversations.create()
base_url = legacy_url[:-len("/api/v1")] + "/api/v2"
client = openai.OpenAI(base_url=base_url, **options)
assert client.conversations.retrieve(conversation.id).id == conversation.id
request = {"conversation": conversation.id, "input": "sdk-raw-with-tools", "metadata": {"proof": "python-v2"}, "stream": True}
frames = list(client.responses.create(**request, extra_headers={"Idempotency-Key": "python-v2-first"}))
assert frames[0].type == "response.created" and frames[0].response.status == "queued"
assert frames[-1].type == "response.completed"
assert [frame.sequence_number for frame in frames] == list(range(len(frames)))
assert all("cma_chat" not in frame.type and "reasoning" not in frame.type for frame in frames)
first = frames[-1].response
assert first.conversation.id == conversation.id and first.previous_response_id is None
first_read = client.responses.retrieve(first.id)
assert first_read.output == first.output and first_read.output_text == "Hello from CMA"
assert first_read.metadata == {"proof": "python-v2"}
alias = openai.OpenAI(base_url=f"{base_url}/agents/{conversation.id}", **options)
retry = list(alias.responses.create(**request, extra_headers={"Idempotency-Key": "python-v2-first"}))
assert retry == frames
with client.responses.stream(conversation=conversation.id, model=first.model, input="sdk-helper-text", extra_headers={"Idempotency-Key": "python-v2-second"}) as stream:
    helper_frames = list(stream)
    final = stream.get_final_response()
assert final.status == "completed" and final.output_text == "Hello from CMA"
assert final.previous_response_id is None and final.id != first.id
assert final.conversation.id == conversation.id
second_read = client.responses.retrieve(final.id)
assert second_read.output_text == final.output_text and second_read.output[0].id == final.output[0].id
items = list(client.conversations.items.list(conversation.id, limit=1))
assert len(items) == 4 and all(item.type == "message" for item in items)
assert {first.output[0].id, final.output[0].id} <= {item.id for item in items}
for body, code in [
    ({"conversation": conversation.id, "previous_response_id": first.id, "input": "invalid", "stream": True}, "conversation_previous_response_conflict"),
    ({"conversation": conversation.id, "input": "invalid", "stream": False}, "stream_required"),
    ({"conversation": conversation.id, "input": "invalid", "stream": True, "model": "wrong-profile-model"}, "model_mismatch"),
]:
    try:
        client.responses.create(**body)
    except openai.APIStatusError as error:
        assert error.status_code == 422 and error.code == code, error.body
    else:
        raise AssertionError(f"Expected {code}")
with urlopen(Request(f"{base_url}/responses/{first.id}/events?starting_after=1", headers={"Authorization": "Bearer local-sdk-conformance"}), timeout=30) as reply:
    replay = [json.loads(line[6:]) for line in reply.read().decode().splitlines() if line.startswith("data: ")]
# Compare the wire fields rather than optional defaults added by the SDK models.
assert [frame["type"] for frame in replay] == [frame.type for frame in frames[2:]]
assert [frame["sequence_number"] for frame in replay] == list(range(2, len(frames)))
assert replay[-1]["response"]["id"] == first.id
print(json.dumps({"sdk": "openai-python", "version": openai.__version__, "versioned": 2, "rawFrames": len(frames), "helperFrames": len(helper_frames), "items": len(items), "status": "passed"}))
