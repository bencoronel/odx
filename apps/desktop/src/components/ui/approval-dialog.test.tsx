import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { ApprovalDialog } from "./approval-dialog";

describe("ApprovalDialog", () => {
  it("requires an explicit confirmation and displays the reviewed call", async () => {
    const onConfirm = vi.fn().mockResolvedValue(undefined);
    const onOpenChange = vi.fn();
    render(
      <ApprovalDialog
        open
        busy={false}
        title="Confirm sending to Odoo"
        description={'Entorno Norte · write · res.partner\nArguments: [[1], {"name":"Ada"}]'}
        confirmLabel="Confirm send"
        onOpenChange={onOpenChange}
        onConfirm={onConfirm}
      />,
    );

    expect(screen.getByRole("alertdialog").textContent).toContain("Arguments:");
    expect(onConfirm).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "Confirm send" }));
    await waitFor(() => expect(onConfirm).toHaveBeenCalledTimes(1));
    await waitFor(() => expect(onOpenChange).toHaveBeenCalledWith(false));
  });
});
