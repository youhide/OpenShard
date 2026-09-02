# A wheel notch that leaked out of the shop list

One `bool` answering two different questions — *taken* against *changed* — and
what it cost. The type that ended it is [`../design_panes.md`](../design_panes.md)'s
router.


Scrolling a vendor's catalogue to its last row and rolling once more zoomed the
map. The window never moved and the pointer never left it, so what the player
saw was the shop staying put while the world jumped a zoom step behind it.

The cause is one `bool` answering two different questions. `App::scroll_vendor`
returned *whether the row offset changed*, and `WindowEvent::MouseWheel` chains
`scroll_skills() || scroll_vendor() || zoom()` — so at either end of the list the
offset stopped moving, the answer went `false`, and the chain fell through to the
camera. `App::scroll_skills` had the shape right from the start: it answers
`true` the moment the pointer is over its window, end of list or not. The vendor
now does the same.

### What is still not right about it

- **The chain conflates "taken" with "changed".** The `||` decides who gets the
  notch, and its answer is also what asks for a redraw. Those are two properties,
  and every handler that joins the chain has to know that the first one is the
  one being asked — the defect above is what happens when it answers the second.
  A third scrolling window will make the same mistake unless the two are split.
- **Only the catalogue swallows the wheel, not the whole vendor window.** The
  guard is `catalogue_contains`, so a notch over the window's frame, its buttons
  or its quantity field still reaches the camera. Skills claims its whole window;
  these two windows disagree about what a wheel over a window means, and the
  disagreement is invisible until a player's pointer sits an inch lower.

