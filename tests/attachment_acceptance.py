"""Verify user-supplied attachment content through the production SDK and transports."""

import base64
import json
import uuid

from inference_fixture import require


PNG = (
    "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+a"
    "d1sAAAAASUVORK5CYII="
)

PDF_TEXT = "TANDEM_PDF_ATTACHMENT_CONTENT"


def pdf_fixture():
    content = f"BT /F1 12 Tf 20 100 Td ({PDF_TEXT}) Tj ET".encode()
    objects = [
        b"<< /Type /Catalog /Pages 2 0 R >>",
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] "
        b"/Resources << /Font << /F1 4 0 R >> >> /Contents 5 0 R >>",
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>",
        f"<< /Length {len(content)} >>\nstream\n".encode() + content + b"\nendstream",
    ]
    data = b"%PDF-1.4\n"
    offsets = []
    for index, obj in enumerate(objects, 1):
        offsets.append(len(data))
        data += f"{index} 0 obj\n".encode() + obj + b"\nendobj\n"
    start = len(data)
    data += b"xref\n0 6\n0000000000 65535 f \n"
    data += b"".join(f"{offset:010d} 00000 n \n".encode() for offset in offsets)
    data += f"trailer\n<< /Size 6 /Root 1 0 R >>\nstartxref\n{start}\n%%EOF\n".encode()
    return base64.b64encode(data).decode()


PDF = pdf_fixture()


def attachment_inputs(include_pdf=False):
    marker = "TANDEM_ATTACHMENT_CONTENT_" + uuid.uuid4().hex
    inputs = [
        {"name": "clipboard.png", "mimeType": "image/png", "data": PNG},
        {"name": "dropped notes.txt", "mimeType": "text/plain",
         "data": base64.b64encode(marker.encode()).decode()},
    ]
    if include_pdf:
        inputs.append({"name": "document.pdf", "mimeType": "application/pdf", "data": PDF})
    return inputs, marker


def require_attachment_request(fixture, offset, marker, include_pdf=False):
    requests = fixture.conversation_requests()[offset:]
    require(bool(requests), "Attachment send did not reach inference")
    encoded = json.dumps(requests[0][1])
    require(PNG in encoded, "The image bytes did not reach the provider request")
    require(marker in encoded, "The dropped text file content did not reach the model")
    if include_pdf:
        require(PDF in encoded or PDF_TEXT in encoded,
                "The PDF content did not reach the provider request")
    require(any(kind in encoded for kind in ('image_url', 'input_image', '"image"')),
            "Image was not sent as native multimodal content")
    return requests


def require_saved_attachments(app, session, inputs):
    messages = app.command("messages", sessionId=session["id"])
    users = [message for message in messages if message["role"] == "user"]
    require(bool(users), "Attachment message was not persisted")
    metadata = users[0]["attachments"]
    require(len(metadata) == len(inputs), "History lost attachment metadata")
    for item, original in zip(metadata, inputs):
        require(item["name"] == original["name"], "History lost the attachment filename")
        require(item["mimeType"] == original["mimeType"], "History lost the attachment MIME type")
        require(item["size"] == len(base64.b64decode(original["data"])),
                "History attachment size differs from its contents")
        require("data" not in item, "Ordinary message history contains attachment bytes")
        saved = app.command("get_attachment", sessionId=session["id"], attachmentId=item["id"])
        require(saved == original, "Persisted attachment contents changed")
    require(PNG not in json.dumps(app.command("snapshot")),
            "Ordinary snapshot contains attachment bytes")
    return metadata


def check_attachments(app, fixture, project, provider, model):
    """Chat and Work share persisted bytes and keep their existing tool boundaries."""
    states = []
    include_pdf = provider["protocol"] != "openai-completions"
    for project_id in (None, project["id"]):
        session = app.command("create_session", projectId=project_id,
                              providerId=provider["id"], modelId=model)
        inputs, marker = attachment_inputs(include_pdf)
        if project_id is not None:
            inputs[1]["mimeType"] = "application/octet-stream"
        offset = len(fixture.conversation_requests())
        events = app.turn(session, "", attachments=inputs)
        requests = require_attachment_request(fixture, offset, marker, include_pdf)
        if project_id is None:
            names = {tool.get("function", tool).get("name")
                     for tool in requests[0][1].get("tools", [])}
            require(names == {"web_fetch"}, "Attaching files expanded Chat's tools")
            require(not any(event["kind"] == "permission" for event in events),
                    "Chat attachments unexpectedly requested tool permissions")
        metadata = require_saved_attachments(app, session, inputs)
        app.wait_title(session["id"], events)
        states.append((session, inputs, marker, metadata))

    chat, _inputs, _marker, metadata = states[0]
    work = states[1][0]
    try:
        app.command("get_attachment", sessionId=work["id"], attachmentId=metadata[0]["id"])
    except RuntimeError:
        pass
    else:
        raise AssertionError("Attachment retrieval ignored conversation ownership")

    before = app.command("messages", sessionId=chat["id"])
    offset = len(fixture.conversation_requests())
    invalid_inputs = [
        {"name": "../escape.txt", "mimeType": "text/plain", "data": "YQ=="},
        {"name": "broken.png", "mimeType": "image/png", "data": "not base64"},
        {"name": "archive.zip", "mimeType": "application/zip", "data": "UEsA/w=="},
        {"name": "unsupported.bmp", "mimeType": "image/bmp", "data": "Qk0="},
    ]
    if not include_pdf:
        invalid_inputs.append({"name": "unsupported.pdf", "mimeType": "application/pdf",
                               "data": PDF})
    for invalid in invalid_inputs:
        try:
            app.command("send_message", sessionId=chat["id"], messageId=str(uuid.uuid4()),
                        content="", attachments=[invalid])
        except RuntimeError:
            pass
        else:
            raise AssertionError("An invalid attachment was accepted")
    require(app.command("messages", sessionId=chat["id"]) == before,
            "Invalid attachments added confirmed history")
    require(len(fixture.conversation_requests()) == offset,
            "Invalid attachments reached inference")
    return states


def check_attachment_resume(app, fixture, states):
    for session, inputs, marker, metadata in states:
        require(require_saved_attachments(app, session, inputs) == metadata,
                "Restart changed attachment identities")
        offset = len(fixture.conversation_requests())
        app.turn(session, "fixture:chat Continue discussing the attached image and notes.")
        require_attachment_request(fixture, offset, marker,
                                   any(item["mimeType"] == "application/pdf" for item in inputs))


def check_stdio_attachments(server, fixture, options):
    """Exercise both remote preparation and direct-send APIs with real SDK inference."""
    include_pdf = options["provider"]["protocol"] != "openai-completions"
    for kind in ("chat", "work"):
        current = {**options, "sessionId": str(uuid.uuid4()),
                   "mode": {"kind": kind}}
        if kind == "work":
            current["mode"]["directory"] = str(fixture.workspace)
        server.command("start_agent", options=current, resume=False)
        inputs, marker = attachment_inputs(include_pdf)
        if kind == "work":
            inputs[1]["mimeType"] = "application/octet-stream"
        offset = len(fixture.conversation_requests())
        prepared_id = str(uuid.uuid4())
        server.command("prepare_agent_message", sessionId=current["sessionId"],
                       preparedId=prepared_id, content="", attachments=inputs)
        server.prepared_turn(current["sessionId"], prepared_id)
        require_attachment_request(fixture, offset, marker, include_pdf)
        inputs, marker = attachment_inputs(include_pdf)
        offset = len(fixture.conversation_requests())
        server.turn(current["sessionId"], "fixture:chat Read the new attachments.",
                    attachments=inputs)
        require_attachment_request(fixture, offset, marker, include_pdf)
        server.command("disconnect_agent", sessionId=current["sessionId"])
