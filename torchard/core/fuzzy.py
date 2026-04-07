"""Simple fuzzy matching for filtering lists."""

from __future__ import annotations


def fuzzy_match(query: str, text: str) -> int | None:
    """Return a score if query fuzzy-matches text, or None if no match.

    Characters in query must appear in text in order, but not consecutively.
    Lower score = better match. Consecutive character matches are rewarded.
    """
    query = query.lower()
    text = text.lower()

    if not query:
        return 0

    qi = 0
    score = 0
    last_match = -2  # position of last matched character

    for ti, ch in enumerate(text):
        if qi < len(query) and ch == query[qi]:
            # Reward consecutive matches (gap of 1)
            if ti == last_match + 1:
                score -= 1  # bonus for consecutive
            else:
                score += ti  # penalty for distance from start / gaps
            last_match = ti
            qi += 1

    if qi < len(query):
        return None  # not all characters matched

    return score
