import { useState } from 'react';
import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { expect, test } from 'vitest';

import { BookmarkPage } from './BookmarkPage';

const BOOKMARK_PAGE_SIZE = 20;

test('bookmark pagination replaces the bounded page instead of accumulating cards', async () => {
  const user = userEvent.setup();
  const items = Array.from({ length: BOOKMARK_PAGE_SIZE + 5 }, (_, index) => `bookmark-${index}`);
  function PageHarness() {
    const [older, setOlder] = useState(false);
    return (
      <BookmarkPage
        items={older ? items.slice(BOOKMARK_PAGE_SIZE) : items.slice(0, BOOKMARK_PAGE_SIZE)}
        hasPrevious={older}
        hasNext={!older}
        onPrevious={() => setOlder(false)}
        onNext={() => setOlder(true)}
      >
        {(page) => page.map((item) => <div key={item}>{item}</div>)}
      </BookmarkPage>
    );
  }
  render(
    <PageHarness />
  );

  expect(screen.getAllByText(/^bookmark-/)).toHaveLength(BOOKMARK_PAGE_SIZE);
  expect(screen.getByText('bookmark-0')).toBeInTheDocument();
  expect(screen.queryByText(`bookmark-${BOOKMARK_PAGE_SIZE}`)).not.toBeInTheDocument();

  await user.click(screen.getByRole('button', { name: 'Next page' }));

  expect(screen.getAllByText(/^bookmark-/)).toHaveLength(5);
  expect(screen.queryByText('bookmark-0')).not.toBeInTheDocument();
  expect(screen.getByText(`bookmark-${BOOKMARK_PAGE_SIZE}`)).toBeInTheDocument();
  expect(screen.queryByText('Page 2 of 2')).not.toBeInTheDocument();
});
