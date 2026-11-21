import {
  combineTransactionSteps,
  findChildrenInRange,
  type Editor,
  type Range,
} from '@tiptap/core';
import { isHistoryTransaction } from '@tiptap/pm/history';
import { Fragment, Slice, type MarkType, type Node } from '@tiptap/pm/model';
import { Plugin, PluginKey } from '@tiptap/pm/state';
import { ReplaceStep } from '@tiptap/pm/transform';
import { bareDocumentLinks, bareLinkPrefix, documentLinkDelimiter } from './documentLinkBoundary';

type Options = {
  editor: Editor;
  type: MarkType;
  defaultProtocol: string;
  validate: (href: string) => boolean;
  shouldAutoLink: (value: string) => boolean;
};

function withoutLinks(fragment: Fragment, type: MarkType): Fragment {
  const children: Node[] = [];
  fragment.forEach((node) =>
    children.push(
      node.isText
        ? node.mark(type.removeFromSet(node.marks))
        : node.copy(withoutLinks(node.content, type)),
    ),
  );
  return Fragment.from(children);
}

export function documentAutolink({
  editor,
  type,
  defaultProtocol,
  validate,
  shouldAutoLink,
}: Options): Plugin {
  return new Plugin({
    key: new PluginKey('documentAutolink'),
    props: {
      transformPasted: (slice, _view, plain) =>
        plain
          ? new Slice(withoutLinks(slice.content, type), slice.openStart, slice.openEnd)
          : slice,
      handlePaste(view, _event, slice) {
        if (!editor.isEditable || view.state.selection.empty) return false;
        const { selection, schema, doc } = view.state;
        if (
          selection.$from.parent.type.spec.code ||
          (schema.marks.code && doc.rangeHasMark(selection.from, selection.to, schema.marks.code))
        )
          return false;
        let explicitLink = false;
        slice.content.descendants((node) => {
          if (type.isInSet(node.marks)) explicitLink = true;
        });
        if (explicitLink) return false;
        const text = slice.content.textBetween(0, slice.content.size, '\n', '\ufffc');
        const link = bareDocumentLinks(text, defaultProtocol).find((item) => item.value === text);
        if (!link || !validate(link.href) || !shouldAutoLink(link.value)) return false;
        return editor.commands.setLink({ href: link.href });
      },
    },
    appendTransaction(transactions, oldState, newState) {
      if (
        !editor.isEditable ||
        transactions.some(
          (transaction) =>
            transaction.getMeta('preventAutolink') || isHistoryTransaction(transaction),
        )
      )
        return;
      const transform = combineTransactionSteps(oldState.doc, [...transactions]);
      const ranges: Range[] = [];
      transform.steps.forEach((step, index) => {
        if (!(step instanceof ReplaceStep) || !step.slice.size) return;
        const mapping = transform.mapping.slice(index + 1);
        const from = mapping.map(step.from, 1);
        const to = mapping.map(step.from + step.slice.size, -1);
        if (from < to) ranges.push({ from, to });
      });
      const pasted = transactions.some((transaction) => transaction.getMeta('uiEvent') === 'paste');
      const tr = newState.tr;
      for (const range of ranges) {
        if (
          !pasted &&
          !documentLinkDelimiter.test(newState.doc.textBetween(range.from, range.to, '\n', ' '))
        )
          continue;
        for (const { node, pos } of findChildrenInRange(
          newState.doc,
          range,
          (node) => node.isTextblock,
        )) {
          if (node.type.spec.code || !node.type.allowsMarkType(type)) continue;
          const text = node.textBetween(0, node.content.size, undefined, '\ufffc');
          for (const link of bareDocumentLinks(text, defaultProtocol)) {
            const from = pos + 1 + link.start;
            const to = pos + 1 + link.end;
            const boundary = link.end + bareLinkPrefix(text.slice(link.end)).length;
            const complete =
              boundary < text.length || pasted || range.to > pos + 1 + node.content.size;
            if (!complete || range.from >= pos + 1 + boundary + 1 || range.to <= from) continue;
            if (
              !validate(link.href) ||
              !shouldAutoLink(link.value) ||
              tr.doc.rangeHasMark(from, to, type)
            )
              continue;
            if (
              newState.schema.marks.code &&
              tr.doc.rangeHasMark(from, to, newState.schema.marks.code)
            )
              continue;
            tr.addMark(from, to, type.create({ href: link.href }));
          }
        }
      }
      return tr.steps.length ? tr : undefined;
    },
  });
}
