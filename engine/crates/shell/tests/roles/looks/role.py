"""A test role whose question shows each item with a sentence and a picture."""

from jaynes_role import Role, Stop

role = Role("looks", version="1.0.0")
role.step("looks", executor="agent", reach={"project": "read"},
          stop=Stop(verdicts=["keep", "redo", "drop"], about="Which looks go on?"),
          instructions="Make looks.", schema={"type": "object"})


@role.items("looks")
def looks(output):
    return [{"id": x["id"], "label": x["name"], "content": x, "detail": x.get("why", ""), "pictures": [x["picture"]]}
            for x in (output or {}).get("looks", [])]


if __name__ == "__main__":
    role.serve()
