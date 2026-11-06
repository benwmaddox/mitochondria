// TodoMVC in Mitochondria: every page works without this script. It adds what the app spec asks of a browser:
// a checkbox submits its form when ticked, double-click edits a todo, Enter or leaving the field saves, Escape cancels.
document.addEventListener('change', e => {
	if (e.target.matches('.toggle, .toggle-all')) e.target.form.requestSubmit();
});
document.addEventListener('dblclick', e => {
	const li = e.target.closest('.todo-list li');
	if (!li || !e.target.matches('label')) return;
	li.classList.add('editing');
	const input = li.querySelector('.edit');
	input.focus();
	input.setSelectionRange(input.value.length, input.value.length);
});
document.addEventListener('keydown', e => {
	if (e.key !== 'Escape' || !e.target.matches('.edit')) return;
	e.target.value = e.target.defaultValue;
	e.target.closest('li').classList.remove('editing');
});
document.addEventListener('focusout', e => {
	if (e.target.matches('.edit') && e.target.closest('li').classList.contains('editing')) e.target.form.requestSubmit();
});
